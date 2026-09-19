# Validation record

Host verification runs on Windows with Git Bash. No board was flashed, erased, reset, probed or accessed over serial. The connected board remains reserved for the Go developer's experiments.

## Host checks

`CARGO_TARGET_DIR=target/login-fix make check` runs formatting, Clippy with warnings denied, 44 Rust tests and a real local HTTP smoke. The separate output directory avoids the running earlier Rust executable's Windows file lock; it does not stop the user's server.

Tests cover checked ledger invariants, authorization, capacities, purchases, reversals, history eviction, persistence, ambiguous writes, file locking, partial-tail recovery and corrupt-frame refusal. Authentication cases cover password hashing, PKCE challenge/redirect/code/session expiry, rate limits, logout, role/status/password revocation, last-Nana protection, legacy migration and durable HTTP retries across restart.

The allocation test observes zero allocations in its tested steady-state domain/JSON path after startup. Worst-case escaped JSON tests cover response bounds and oversized journal-event refusal. This does not measure HTTP libraries, TLS, FreeRTOS, Wi-Fi, NVS or board stacks.

Fifteen offer tests derive the behavior from `../nanacoin/internal/core/offers_test.go` and add Rust persistence/capacity cases. They cover both payment directions, negotiated prices, permissions/privacy, insufficient funds and disabled accounts, exact/configurable deadlines, replay, clock rollback, correction overdrafts, history eviction, pinned listings, closed-slot recycling, bounded input, durable keyed retries, and injected failed/ambiguous accept and undo writes. A second allocation test fills all 32 offer slots, reads them 1,000 times and retries an acceptance 1,000 times with zero allocations in the API/domain path after initialization.

The HTTP smoke uses disposable journals and processes on an unused loopback port. It exercises JSON-only root behavior, Angular-shaped login/views, provisioning, CORS, members, money movement, listing edits, offers/accept/unaccept, 1,000 consecutive offer reads, revocation, body limits, restart and durable retries. It invokes the migration CLI against a generated legacy Rust journal and checks that balances survive while the old token stops authenticating. It never migrates the user's real journal. This is API contract testing, not an interactive Angular browser test.

## Firmware compilation

The compile-only release build and link passed for xtensa-esp32s3-espidf, ESP-IDF v5.5.3 and mDNS 1.8.2. Output:

```text
C:/ncr/xtensa-esp32s3-espidf/release/nanacoin-esp32
```

Validation uses placeholder Wi-Fi values and ignored local development TLS files. No administrator credential is embedded. SDK configuration enables HTTPS, octal PSRAM/malloc, PSRAM-backed NVS cache, a 64 KiB main stack and multicore FreeRTOS. Wi-Fi/lwIP uses core 0; HTTPS selects core 1.

Firmware also starts ESP-IDF SNTP for persisted offer deadlines. Clock synchronization and settlement behavior across physical power cycles still require board validation. No test used the plugged-in device.

## Deferred hardware acceptance

Compilation does not establish board uptime or OOM immunity. Future board work, after authorization, must verify boot/PSRAM, TLS/certificate trust, mDNS, authentication, NVS persistence/interrupted writes, Wi-Fi recovery, fragmentation during repeated TLS handshakes and stack high-water marks.

`scripts/soak.py` is a future two-client HTTPS read soak with bounded response reads, fresh connections and latency/failure reporting. It prompts for username and PIN/password and logs in through PKCE; NANACOIN_USERNAME / NANACOIN_PASSWORD environment values are optional. It changes no household funds. Keep duration below the eight-hour session lifetime. It has not been run against the board.

Flash endurance, memory headroom and NVS power-loss behavior remain unmeasured. Backup and compaction are required before long-term use.
