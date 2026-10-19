# Deploy Game Matcher to the Waveshare C6

## The board

| Item | Value |
|---|---|
| Model | Waveshare ESP32-C6-LCD-1.47, **non-Touch** ([board notes](../BOARD_SKILL_ESP32_C6_LCD_1_47.md)) |
| Chip | ESP32-C6FH8, one RISC-V core, 512 KiB SRAM, no PSRAM |
| Flash | 8 MiB (probed) |
| Base MAC | `ac:eb:e6:1e:13:40` (formerly minicloud) |
| USB | VID:PID `303A:1001`, native USB Serial/JTAG; COM17 when this was written |
| Hostname | `gamematcher.local` |

Find the port by VID:PID, never by habit; exclude COM3 (Intel AMT). The
deploy script refuses any board whose MAC or flash size differs.

## Flash layout

| Partition | Offset | Size | Kept by `make update` |
|---|---|---|---|
| nvs | `0x9000` | 24 KiB | yes |
| phy_init | `0xf000` | 4 KiB | yes |
| factory (app + UI) | `0x10000` | 2.5 MiB | rewritten |
| coredump | `0x290000` | 64 KiB | yes |
| gmstate (accounts, sessions) | `0x2A0000` | 512 KiB | yes |

## Rules

1. **`make flash` erases everything.** Use it for the first install (the
   repave from minicloud, authorized by the owner on October 17, 2026) and
   never again unless the partition table changes.
2. **`make update` for every later change.** It reads the board's partition
   table first and refuses unless it is exactly Game Matcher's.
3. **Agents ask before flashing.** The owner said yes to the Sprint 0 repave;
   each later flash is its own question.
4. **Claim admin immediately after a first install.** Whoever submits the
   setup form first becomes admin. Open http://gamematcher.local/ yourself
   right after flashing (or build with `GAMEMATCHER_SETUP_CODE`).

## Steps

```sh
cd gamematcher
make certs                         # once
make firmware                      # TRANSPORT=http2 default; GATE=1 only for the gate
make flash PORT=COM17              # first install
make update PORT=COM17             # afterwards
```

Then check, by IP if `.local` doesn't resolve:

```sh
curl -s http://gamematcher.local/api/v1/status?fmt=json
curl -s --cacert api/certs/household-ca.crt https://gamematcher.local/api/v1/status?fmt=json
curl -s http://gamematcher.local/api/v1/log.txt | tail -20
```

## Diagnostics

- `/api/v1/sys?fmt=json`: heap (free, minimum ever, largest block), TLS
  handshakes/failures, open connections, reset reason.
- `/api/v1/log.txt`: the board's own log ring, including the previous
  boot's crash report from the core dump. Prefer it to serial captures.
- `/metrics`: the same as Influx lines, plus `gm_accounts`, `gm_sessions`,
  `gm_commits`, `gm_snapshot_bytes` (housemetrics can scrape it).
