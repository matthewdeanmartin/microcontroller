# NanaCoin Rust

A JSON-only household scrip API targeting ESP32-S3 N16R8. The client is the existing `nanacoin_web` / Angular application, running locally on port 4200. This project serves no HTML. The Rust domain uses typed commands, identity newtypes, checked integer amounts and explicit errors; a wire adapter supports the existing client's principal flows.

## Run locally

From Git Bash:

```bash
cd nanacoin_rs
make run
```

The API listens on `http://127.0.0.1:8080`. Use the Angular client at `http://localhost:4200`; `/` on the API returns JSON 404. A fresh journal is unprovisioned: use the client's household setup and choose a username and PIN/password.

If you ran the earlier Rust prototype, stop that Rust server first, then:

```bash
make migrate-login
make run
```

The interactive migration reads the old credential from ignored `.local/admin-token` (or prompts for it), asks for a username and PIN/password, and appends a migration event. Balances, member identity and history are preserved. It does not delete the journal. Repeat for other legacy accounts with their credentials. This only migrates the earlier Rust prototype, not Go journals.

`NANACOIN_PORT` and `NANACOIN_JOURNAL` override the port and default `nanacoin.journal` path. An exclusive file lock prevents two writers. Desktop transport is loopback HTTP. `NANACOIN_ORIGINS` sets a comma-separated exact CORS allowlist. Defaults include localhost and 127.0.0.1 on port 4200 and HTTP/HTTPS `nanacoin.local` for the separate client board. Add the actual client origin if different.

## Authentication and compatibility

Authentication follows the existing server: salted PBKDF2-HMAC-SHA256 passwords, its 1,000-round work factor and minimum four-byte PIN, S256 PKCE with one-use 60-second codes, and eight-hour RAM-only sessions. Restart clears sessions. Logout, password changes, role changes and disabling a member revoke the relevant sessions. Five failed password attempts lock the username for five minutes. Roles and last-active-Nana protection are enforced in the domain. There are no permanent account access tokens.

Implemented client flows include provisioning/login/logout, status, members and role/status/password updates, balances and recent transactions, issue/retire/transfer/reverse, listings with descriptions and edits, purchase/cancel, and household configuration. Money endpoints use durable `Idempotency-Key` receipts. The typed command endpoint also remains available; credential commands cannot be submitted through it.

Negotiated offers support proposing, accepting, declining, withdrawing and undoing acceptance. Only the listing owner accepts, at the offered price; SELL charges the offerer and BUY charges the listing owner. Proposals need no funds, while acceptance does. Offers are private to the two parties and Nana. Undo is available to either party or Nana until the configured deadline (48 hours by default), creates a reversal even if the payee spent the money, and reopens the listing. Refund, status and reopening commit in one event. Acceptance details survive recent-history eviction, so an unexpired deal can still be undone.

The September 19 parity pass adds USD wallets, a bounded forex quote book, external-currency listing metadata, account/listing reads, balance/ledger privacy, and member/listing timestamps. Recent transactions retain 365 records in a preallocated ring; HTTP ledger pages cap at 100. Old timestamp-less Rust events still report zero. Diagnostic/log streaming and Go journal import remain absent. Loans and interest are not implemented by either current server. See [PARITY.md](PARITY.md) for remaining differences. Direct purchases still pay immediately. Nana's manual reversal requires a retained transaction, permits correction overdrafts, and does not reopen listings; reversing an offer's payment prevents a second refund through unaccept.

Settlement deadlines use server wall time, never browser time or uptime. Firmware starts SNTP after Wi-Fi; optionally set `NANACOIN_NTP_SERVER` at build time for a reachable LAN time server. Until the clock is valid, timed offer mutations return 503 and offers are not advertised as reversible. A clock behind the last durable event also blocks timed mutations. Deadlines and the configuration survive replay; changing the window applies only to future acceptances.

## Build and check

```bash
make check       # format, Clippy, Rust tests and real local HTTP smoke
make help
make certs       # development TLS files; preserves existing keys
make firmware   # compile only; requires Wi-Fi settings
```

Firmware targets ESP-IDF v5.5.3, esp-idf-svc 0.52.1 and managed mDNS 1.8.2. The application is Rust over ESP-IDF's C platform libraries. The script uses the installed Espressif Rust toolchain and, on this Windows machine, the SDK under `C:/Espressif`. Firmware output defaults to short path `C:/ncr` to avoid SDK path-length failures; override `CARGO_TARGET_DIR` if needed. Host output normally uses `target`.

```bash
export NANACOIN_WIFI_SSID='Your 2.4GHz Wi-Fi'
export NANACOIN_WIFI_PASSWORD='Your Wi-Fi password'
make firmware
```

Wi-Fi credentials come from `NANACOIN_WIFI_SSID` / `NANACOIN_WIFI_PASSWORD` when
those are exported. Otherwise `build.rs` reads the first gitignored `.env` or
`config.py` that defines `WIFI_SSID` / `WIFI_PASSWORD` (this crate first, then
`nanacoin_web`, then the MicroPython projects), so `make firmware` works with no
environment setup. The build prints which file it used, never the value, and the
environment always wins. Wi-Fi credentials and ignored `certs/server.crt` / `certs/server.key` are embedded during compilation. There is no embedded household password or admin token. Treat binaries and build caches as secrets. The build does not flash, reset, probe or open serial connections. **This pass builds only; flashing waits until the owner connects the board.**

The intended board API address is `https://nanacoin-rs.local/api/v1/status`. mDNS uses local multicast, rather than requiring a router-assigned name. Clients need mDNS support and multicast reachability. Trust the development certificate or supply a trusted LAN certificate matching this hostname. No plaintext HTTP fallback runs on the board. The client stays on its own host with its origin allowed by CORS. Certificate renewal is manual.

## Memory and persistence

Rust does not automatically prevent fragmentation. Application collections have explicit limits and return capacity errors:

| Resource | Limit |
|---|---|
| Members / listing slots / recent transactions | 16 / 48 / 365 |
| Forex quotes | 16; recycle closed/expired quotes, never live quotes |
| Offer slots / offer message and undo reason | 32 / 140 UTF-8 bytes |
| Names / titles / descriptions and memos | 40 / 80 / 96 UTF-8 bytes |
| Request body / reused response buffer | 1 KiB / 512 KiB |
| Sessions / pending login codes | 64 / 16 |
| Journal | 4,096 fixed 1,024-byte records |
| Durable HTTP retry index | 4,096 entries allocated once; 192 KiB on 64-bit host |
| HTTPS sockets / handler stack | 4 / 24 KiB |
| Startup stack | 64 KiB |
| Movement amount | 1 through 1,000,000,000 whole coins |

Allocation tests observe zero allocations after startup across 2,999 financial writes with state reads, and across 1,000 forex trades with retries and quote reads. The larger response buffer is allocated once for the 365-record state view and worst-case JSON escaping; it does not grow per request. The history backing storage also allocates once and overwrites oldest records without shifting the full history. TLS, HTTP headers, Wi-Fi and NVS still allocate. PSRAM and PSRAM-backed NVS cache are enabled, with 64 KiB reserved for internal-RAM allocations. Heap metrics include largest free block. Runtime OOM resilience requires hardware measurement.

Offer slots recycle the oldest closed or settled deal, never an open or still-reversible deal. Reversible acceptances also pin their listing slot. Fixed domain state is boxed once at startup so moving the service does not copy the whole state through the firmware stack. Views serialize directly from bounded iterators, without constructing response vectors. A regression test covers a full table, 1,000 offer reads and 1,000 acceptance retries without API/domain allocations after setup.

Wi-Fi/lwIP and the diagnostics sampler use core 0; HTTPS and domain work use core 1. Financial writes have one owner. The service mutex now covers only the domain call: each httpd worker owns a response buffer, so serializing a reply and writing it to the socket happens outside the lock. Four TLS sockets are served concurrently; requests beyond that queue rather than being refused, so slow clients still delay others.

`GET /api/v1/diag` is unauthenticated, takes no lock, and is answered from the core 0 sampler's atomics, so it responds while a write is in flight. It reports uptime, internal free/largest/minimum heap, free PSRAM, the sample count and which core owns each role. mbedTLS content buffers are 4 KiB in / 2 KiB out: request bodies are capped at 1 KiB, and the previous 16 KiB input buffer could not be allocated four times inside the reserved internal RAM.

Validated events append durably before state changes. Failed or ambiguous writes latch the service unavailable until replay. Complete corrupt records fail startup; only an incomplete final desktop frame is truncated. NVS initialization errors do not authorize automatic erasure. The firmware's 8 MiB NVS partition and overall layout differ from Go.

After 4,096 commands, writes refuse further changes and reads remain available. There is no compaction, export or automatic erase. Add backup/compaction before long-term use. Amounts and IDs stay within JavaScript's exact integer range; money never uses floating point.

USD issuance is Nana-only. Quotes support BID/ASK, integer cents per coin, all-or-nothing takes, expiry and owner/Nana cancellation. A take validates both wallets and records both currency legs in one durable event. Balances and quote status replay together; there is no half-trade state. Ordinary disabled accounts cannot send or receive. Currency listings retain descriptive metadata but do not themselves move USD wallets.

See [API.md](API.md), [PARITY.md](PARITY.md) and [VALIDATION.md](VALIDATION.md).
