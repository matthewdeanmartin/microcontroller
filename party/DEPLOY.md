# Deploying party

## Board identity

| | |
|---|---|
| Board | ESP32-S3 N16R8 (16 MiB flash, 8 MiB octal PSRAM) |
| MAC | `ac:a7:04:2c:29:9c` (the household's former mastomini S3, retired 2026-10-21; native USB, COM11 on the build PC) |
| Hostname | `party.local` |
| State | `partydata` NVS partition (7.5 MiB), never erased by firmware |
| Address | 192.168.1.161 (DHCP) |

The household's other N16R8s run the bots and NanaCoin (mastomini moved to
an ESP32-P4). Never
flash one of those. `tools/deploy-s3.py` has no default MAC on purpose:
`make flash` and `make update` need `MAC=` and refuse a board that doesn't
match.

## Rules for agents

- Ask the owner before any flash. Read the MAC first.
- After the first install, use `make update` (app only). `make flash` erases
  the whole board.
- Read the board's `/api/v1/log` and `/api/v1/sys` rather than long serial
  captures.

## Deployments

- **2026-10-21, first install.** `make flash PORT=COM11 MAC=ac:a7:04:2c:29:9c`
  on the S3 that ran mastomini until that day (the household moved to the
  ESP32-P4; its data is on the P4 and in an encrypted export, so the owner
  declined a 16 MB backup: reading flash over this USB takes far too long).
  `make test` green (45 Rust, 10 UI). Plain HTTP build `6f335bf+`, app
  1,696,096 bytes (40.4% of app0), Wi-Fi from the environment,
  `PARTY_SETUP_CODE` from `party/.env`. Erase 6.4 s, app written in 9.4 s,
  booted without a manual reset. `/api/v1/status`: setup needed, setup code
  required, clock synced. `/api/v1/sys`: 93 KB internal free (largest block
  31 KB), 7.6 MB PSRAM free, RSSI -70 dBm. Not yet claimed by the admin;
  the Sprint 0 gate (`GATE=1`) has not been run.

- **2026-10-21, Sprints 3-8, app-only.** `make update PORT=COM11 MAC=ac:a7:04:2c:29:9c`.
  Plain HTTP, app 2,417,040 bytes (57.6% of app0), written in 14.5 s, verified,
  reset by the script. Afterwards: `/api/v1/status` has the new fields,
  `/api/v1/events` answers, health ok, and "Every record loaded" with 2 people and 1
  group kept. `/api/v1/sys` at 6 s uptime: 67.6 KB internal free (was 78.9 KB
  before; the worker's 20 KB internal stack), largest block 31 KB (unchanged),
  7.6 MB PSRAM free. The build string still reads `6f335bf+`, because the work isn't
  committed. Email, the bot and publication aren't set up yet.

- **2026-10-21, status light, app-only.** The framework's status light can now
  drive a WS2812 pixel (`Led::ws2812`, RMT, mastomini's timing). party lights GPIO 48:
  dim blue is "on", amber means connecting (no Wi-Fi), red means trouble.
  It shows the power-on code (1.5 s on, then 1–6 flashes for the reset reason), a
  failed startup step's number, and while healthy "It is party time!" in Morse.
  App 2,437,280 bytes (58.1%), written in 15.3 s, verified. Boot log: `Status WS2812
  pixel: reset "USB" (2 flashes)`; `/api/v1/sys` light status `healthy`; health ok.

## Build

```sh
make firmware                 # plain HTTP; Wi-Fi from .env (PARTY_WIFI_SSID / WIFI_SSID ...)
make firmware TRANSPORT=http2 # needs `make certs` first (party.local from the household CA)
make firmware GATE=1          # Sprint 0 measurement build, never a release
```

The image, bootloader, partition table and `deployment-images.json` land in
`api/.embuild/`. `manifest.py` checks the partition table against
`api/partitions.csv` and the app's size against `app0` (4 MiB).

## First install

```sh
make flash PORT=COMn MAC=xx:xx:xx:xx:xx:xx
```

Then, straight away, open `http://party.local/` and claim the admin account.
The first visitor becomes admin. Optionally build with `PARTY_SETUP_CODE` to
gate that.

## Update

```sh
make update PORT=COMn MAC=xx:xx:xx:xx:xx:xx
```

It refuses unless the board already has party's exact partition table. NVS
and `partydata` survive.

## Sprint 0 gate

With a `GATE=1` build on the board (the endpoints are open, which is why it is
never a release):

```sh
curl -X POST 'http://party.local/api/v1/gate/storage?records=20000&bytes=200'
curl -X POST 'http://party.local/api/v1/gate/crypto?bytes=262144'
curl -X POST 'http://party.local/api/v1/gate/outbound?secs=300'   # while a bench loads the server
curl 'http://party.local/api/v1/gate'                             # results
curl 'http://party.local/api/v1/sys?fmt=json'                     # heap, largest free block
```

Record the results in [spec/measurements/s3-gate.md](spec/measurements/s3-gate.md).
