# Validation â€” 2026-09-26

- `cargo test --locked`: six tests passed (delayed upstream / responsive
  admission and polling, full capacity, NTP nonce rejection, terminal failure,
  silent upstream timeout, and completed-only expiration).
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `bash -n scripts/build-esp32.sh scripts/deploy.sh`: passed.
- Deployment `--dry-run`: passed without opening a serial port.
- `bash scripts/build-esp32.sh`: compiled `xtensa-esp32s2-espidf` with Rust
  `esp`, ESP-IDF 5.5.3, and esp-idf-svc 0.52.1. Application image: **977,808
  bytes** against a 1,572,864-byte partition. Native USB CDC and the S2 target
  confirmed in generated sdkconfig.
- Existing Wi-Fi configuration discovered in
  `../archive/nanacoin_web/config.py`; credentials were not copied into source.
- Actual desktop HTTP requests: `/` returned the 4,638-byte website,
  `/api/health` returned 200, and job submission returned **202 in 16 ms**
  with Location and Retry-After. Polling reached `succeeded` following a real
  NIST query to `129.6.15.28:123`, stratum 1, **78 ms UDP round trip**.
  These are one-run observations, not performance guarantees.

## Hardware deployment and runtime — 2026-09-26 (local)

Installed on the attached ESP32-S2FNR2 rev 1.0, MAC `80:65:99:f0:1c:9c`.
The user explicitly authorized skipping the backup because there was no
valuable data on the board. `deploy.py --port COM4 --provision --skip-backup`
wrote the bootloader, partition table, and application; esptool verified the
written hashes. No complete flash backup exists.

The old MicroPython REPL accepted `machine.bootloader()` and changed COM6
to ROM COM4. After flashing and a physical RESET, the Rust application exposed
USB CDC on **COM16** (still VID:PID 303A:0002). Thus VID/PID alone does not
prove that an ESP-IDF device is in ROM bootloader mode.

The board joined Wi-Fi and served **http://192.168.1.157/**:

- `GET /api/health`: 200, capacity eight, initially zero occupied slots.
- `GET /`: 200, 4,638 bytes; expected website content confirmed.
- `POST /api/jobs`: **202**, Location and Retry-After headers, accepted in
  **844 ms** including the LAN exchange.
- Polling that job returned **succeeded**, NIST `129.6.15.28:123`, stratum 1,
  **21 ms upstream round trip**, with a server transmit timestamp.

Several earlier TCP connection attempts timed out. A three-packet ping saw
33% loss, with successful replies taking 140–245 ms. Later HTTP requests
succeeded. These observations show intermittent LAN connectivity; the cause
has not been isolated. Heap, stack margins, long-running stability, browser
rendering, and multi-client load have not been measured.

## Backup investigation

The streaming stub repeatedly stalled at address `0x61000`, including with
a new cable and individually verified 64 KiB chunks. The chunked attempt
saved only the first 393,216 bytes, and stopped on its 45-second timeout.
Those `.partial` files are not usable full-board backups.

ROM reads progressed steadily at approximately 1 KiB/s, but were stopped
cleanly because a full read would take about an hour. Default provisioning
therefore offers the slow ROM backup (two-hour deadline); the user selected
`--skip-backup` for this installation. `--fast-backup` is available for boards
unaffected by [esptool issue 658](https://github.com/espressif/esptool/issues/658).
`--after no_reset_stub` is needed to keep a stub alive between commands;
`--after no_reset` exits the stub and can disrupt native USB enumeration.
