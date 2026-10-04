# Async worker, small-board edition

A Rust website for the **ESP32-S2 Mini**: single core, 4 MiB flash, 2 MiB
PSRAM. Like the Rust NanaCoin and Mastomini projects, the domain logic runs
unchanged on desktop and ESP-IDF. This PoC does not require PSRAM.

The experiment is **HTTP 202 + job tokens + polling**. It uses one bounded
background thread (a FreeRTOS task on the board), rather than a Rust async
executor. Network I/O never happens in the job submission/poll handler or
while the job table mutex is held. Slow HTTP clients can still occupy the
HTTP server task; this is not a general high-concurrency HTTP server.

```mermaid
sequenceDiagram
    Browser->>HTTP server: POST /api/jobs
    HTTP server->>Job table: Reserve slot; enqueue index
    HTTP server-->>Browser: 202 + token + Location
    Worker->>Job table: queued → running
    Worker->>NIST: UDP NTP query
    Browser->>HTTP server: GET /api/jobs/{token}
    HTTP server-->>Browser: 200 queued / running
    NIST-->>Worker: Time sample (or timeout)
    Worker->>Job table: succeeded / failed
    Browser->>HTTP server: GET /api/jobs/{token}
    HTTP server-->>Browser: 200 + result / error
```

## Attached board

The PoC is installed and has completed a real NIST job on the S2 at
**http://192.168.1.157/** (DHCP address, may change). The app USB port was COM16.
See [VALIDATION.md](VALIDATION.md) for measured results and intermittent LAN timeouts.

## Desktop

In Git Bash:

```bash
cd /c/github/microcontroller/async_worker_rs
make run
# Open http://127.0.0.1:8080
make test
```

`WORKER_BIND` defaults to `127.0.0.1:8080`. `WORKER_NTP_ADDR` defaults to
`129.6.15.28:123` (NIST's time-a-g). Set it to another **numeric IP:port**
to use a different server or a local fake NTP server. Bracket IPv6 addresses.
Numeric addresses avoid a DNS lookup whose deadline would be outside our
control. Desktop configuration is read at runtime; firmware configuration
is read at build time. A firmware upstream change requires a rebuild.

[NIST's server list](https://tf.nist.gov/tf-cgi/servers.cgi) documents the
addresses and the minimum four-second interval. This worker waits **five
seconds after each completed attempt**, including failures, before the next
query. Browser polling never calls NIST. Multiple devices/processes do not
coordinate this limit, so use a local upstream for load experiments.

## Build for the S2

The Git Bash script supports this workstation's existing ESP-IDF 5.5.3 and
`esp` Rust toolchain. It puts intermediate files in `C:/awr` to avoid Windows
path limits. Elsewhere, activate ESP-IDF and espup's environment first.
Dependencies/tools: Rust `esp` with rust-src, ldproxy, ESP-IDF 5.5.3,
Python with esptool 4.x, CMake and Ninja.

```bash
make firmware
```

Credentials follow the existing projects' pattern. Environment variables
win: `WORKER_WIFI_SSID` / `WORKER_WIFI_PASSWORD`, then their generic
`WIFI_*`, `NANACOIN_WIFI_*`, and `MASTOMINI_WIFI_*` aliases. File fallback,
nearest first:

1. `.env`, then `config.py` in this project.
2. `../archive/nanacoin_rs/.env`.
3. `../archive/nanacoin_web/config.py`.
4. `../hello_wifi_py/config.py`.
5. `../secret_messages/config.py`.
6. `../hello_wifi_s3_py/config.py`.
7. `../../mastomini/mastomini_rs/.env`.

Files accept literal `KEY=value` or `KEY="value"` / `KEY='value'` lines,
with optional `export`. They are parsed, never executed. Quoted values
should occupy the rest of the line; escapes are kept literally. `config_example.py`
can be copied to gitignored `config.py`. Build messages name the source,
not its contents. Credentials are embedded in firmware and Cargo build
artifacts; keep those private. Wi-Fi must be 2.4 GHz WPA2-compatible.

## Deploy

```bash
make ports
# Hold BOOT, tap RESET, release BOOT; list ports again.
bash scripts/deploy.sh COM_PORT --dry-run
# First installation: replaces existing firmware, after a full flash backup.
bash scripts/deploy.sh COM_PORT --provision
# Later updates: verify the partition table and write only the application.
make deploy PORT=COM_PORT
```

Replace `COM_PORT` with the current board port. The S2 changes ports between
ROM bootloader and application mode. The script fixes `--chip esp32s2` so an
S3 is rejected, does not erase the full chip, and keeps the bootloader alive
between read/write commands. First provisioning saves all 4 MiB under
`firmware/backup-<UTC>.bin`; that directory is ignored because backups can
contain old credentials/data. Backups use the slower ROM reader after a native-USB streaming stall; allow up to two hours (roughly one hour at the measured speed). A completed backup gets a SHA-256 sidecar. It writes bootloader at `0x1000`, partition
table at `0x8000`, and app at `0x10000`. Existing NVS bytes are preserved;
incompatible NVS causes startup to fail rather than silently erasing it.

Firmware uses the ESP-IDF single-app-large layout (1536 KiB factory app).
Build conversion rejects an oversized image. There is no OTA or filesystem.
The website is compiled into the image. Provisioning replaces the previous
application and partition map; the full backup is needed to restore it.
To restore, enter ROM boot mode and use the same esptool Python environment:
`python -m esptool --chip esp32s2 --port COM_PORT write_flash 0x0 firmware/backup-<UTC>.bin`.

Tap RESET if ROM mode persists after flashing. Use the new application port
with `python -m serial.tools.miniterm COM_PORT 115200` and reset to see the
DHCP address printed as `Async worker ready: http://.../`. This PoC does not
register an mDNS hostname. Wi-Fi disconnects trigger reconnection attempts;
an unsuccessful initial connection exits startup.

## API and budgets

| Request | Result |
|---|---|
| `GET /` | Embedded website |
| `POST /api/jobs` (empty body) | 202, random 128-bit token, `Location`, `Retry-After: 1` |
| `GET /api/jobs/{token}` | 200 with queued/running/succeeded/failed; pending replies have `Retry-After: 1` |
| Unknown/expired/restarted token | 404 |
| All eight slots occupied | 503, `Retry-After: 5`; no accepted job evicted |
| `GET /api/health` | Occupied slots and capacity; no upstream I/O |

Responses have `Cache-Control: no-store`. Submission has no parameters; an
HTTP client cannot choose arbitrary outbound destinations. Sample response:

```json
{"id":"<token>","status":"succeeded","result":{"unix_ms":1790000000000,"round_trip_ms":21,"stratum":1,"server":"129.6.15.28:123"},"error":null}
```

Eight slots include queued, running, and retained completed jobs. Completed
jobs expire **120 seconds after completion**, reclaimed on the next API
request. Live jobs never expire or get overwritten. Each UDP send/receive has
a three-second timeout. One job makes one upstream attempt; no automatic
retries. Maximum eight-job backlog is approximately 83 seconds when both
socket operations consume their full timeout. The browser polls for up to
100 seconds and shows transport failures independently from job failures.
Tokens and jobs vanish on restart. Submission is not idempotent: a lost 202
followed by another POST can create another job.

Application state is bounded to eight small records and eight queue indices;
the worker uses a 12 KiB stack, HTTP task 12 KiB, main task 16 KiB. NTP receives
into 512 bytes. JSON and the ~5 KiB page allocate per response; Wi-Fi/ESP-IDF
also allocate, so these are design budgets, not measured free-heap claims.
The S2 HTTP server permits three sockets. No core-1 affinity is configured.

This is a trusted-LAN PoC: plain HTTP, no authentication, no durable queue,
and unauthenticated NTP. NTP validates server mode/version, synchronization,
stratum, and echoed request nonce. A stratum-zero refusal is a failed job.
The result is the server's transmit time plus measured round trip, not an
offset-corrected clock or a change to the board's clock. Replace `ntp::query`
to experiment with another bounded external operation.

## Checks

`cargo test --locked` checks acceptance while upstream is delayed, queue
capacity, reply nonce validation, terminal failures, expiration, and a silent upstream timeout.
`cargo clippy --locked --all-targets -- -D warnings` checks the desktop target.
`make firmware` compiles the actual S2 target and gates image size.
See [VALIDATION.md](VALIDATION.md) for observed results and hardware status.

### S2 readback limitation observed on this board

Stub backups repeatedly stop at address `0x61000`, including with a different
cable and 64 KiB chunks. See [esptool issue 658](https://github.com/espressif/esptool/issues/658).
The default full backup therefore uses ROM reads; this is very slow.
`--fast-backup` is available for boards unaffected by that issue.
If replacing the old firmware without exact rollback is acceptable, explicitly
use `bash scripts/deploy.sh COM_PORT --provision --skip-backup`.
No full backup is created in that mode. It still targets only the S2 and writes
the three compiled images without a full-chip erase.
