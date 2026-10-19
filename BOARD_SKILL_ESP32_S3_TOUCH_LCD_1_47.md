# Board Skill — Waveshare ESP32-S3-Touch-LCD-1.47

Working notes for the **Touch** 1.47-inch Waveshare board, plugged in and
checked October 17, 2026. Nothing has been flashed to it yet: it still runs
Waveshare's factory demo, and a full backup of that is saved (see
[Recovery](#recovery-the-factory-backup)). Companion references:
[C6 LCD 1.47](BOARD_SKILL_ESP32_C6_LCD_1_47.md) (same size screen, no touch,
now running Game Matcher), [S3 Touch LCD 2](BOARD_SKILL_ESP32_S3_TOUCH_LCD_2.md)
(same chip, bigger screen, motion sensor), [S3 N16R8](BOARD_SKILL_ESP32_S3_N16R8.md),
[S2 Mini](BOARD_SKILL_ESP32_S2_MINI.md).

Marked **measured** below = probed on this unit (esptool, the factory boot log).
Everything else comes from Waveshare's pages or the CircuitPython port, linked
at the end.

## The short version

It's the C6 board's bigger sibling: the same 172 × 320 screen size, but with a
**touchscreen, a dual-core 240 MHz S3, 8 MB of PSRAM and 16 MB of flash**, plus
a battery charger. For web apps on this repo's framework that means:

- **RAM stops being the constraint.** The C6 has ~300 KB to work with. This one
  has ~260 KB of internal RAM *plus 8 MB of PSRAM*. Full-screen framebuffers
  (110 KB) and bigger JSON responses fit.
- **Twice the app space.** 16 MB of flash vs 8 MB. The C6's 2.5 MB app
  partition at 89% full is the problem this board doesn't have.
- **Tap instead of BOOT.** The screen is touch, so on-screen "next card" /
  "I'm in" buttons become possible.
- **It can run unplugged** from a LiPo cell (charger on board; no battery came
  with it).
- **No RGB LED.** The C6's light-in-the-gap LED isn't on this board (none
  listed or seen in the factory demo).

## Identity (measured)

```text
Chip:          ESP32-S3 (QFN56), revision v0.2  — sold as ESP32-S3R8
Features:      Wi-Fi, BLE, Embedded PSRAM 8MB (AP_3v3)
CPU:           dual-core Xtensa LX7, 240 MHz (factory log)
Crystal:       40 MHz
USB mode:      USB-Serial/JTAG (native USB, one Type-C port)
Flash:         16 MB, quad (QIO, 80 MHz); manufacturer 0x20, device 0x4018
PSRAM:         8 MB octal (OPI), AP Memory gen 3, 3 V; "memory test OK"
Base MAC:      28:84:85:b1:70:e4
Windows port:  COM5 this session (the C6 is COM17); USB 303A:1001
```

| | This board | C6 LCD 1.47 (Game Matcher) |
|---|---|---|
| CPU | 2 × 240 MHz Xtensa, FPU | 1 × 160 MHz RISC-V, no FPU |
| Flash | 16 MB | 8 MB |
| PSRAM | 8 MB | none |
| Screen | 1.47" 172 × 320, **JD9853**, capacitive touch (AXS5106L) | 1.47" 172 × 320, ST7789, no touch |
| Radio | Wi-Fi 4 (2.4 GHz) + BLE 5 | Wi-Fi 6 (2.4 GHz) + BLE 5 + 802.15.4 |
| SD slot | yes, 4-bit SDMMC (fast) | yes, SPI |
| Battery | LiPo charging (ETA6098) + voltage reading | no |
| RGB LED | no | yes |
| Buttons | BOOT, RESET | BOOT, RESET |

Note the C6 has the newer radio (Wi-Fi 6, Thread/Zigbee capable); the S3 has
everything else.

## What the factory demo does (measured)

Waveshare's demo (`factory`, ESP-IDF 5.3.2, built May 8, 2025) boots in about
1.2 s, of which ~0.7 s is the PSRAM self-test. Then, in order:

1. Calibrates the **battery** reading, starts Wi-Fi as a station and tries to
   join a network it was built with. That network isn't ours, so it logs
   "connect to the AP fail" and keeps retrying. That's harmless.
2. Brings up the **JD9853** display and **touch** controller, starts LVGL.
3. Tries to mount an **SD card** over 4-bit SDMMC. With no card inserted it
   logs a timeout error and carries on.
4. Sets the backlight to **100%**. For the C6's panel Waveshare recommends 50%
   or less (heat/discoloration over time); assume the same here and run
   ours at ~40–50%.
5. Watches the BOOT button.

## Memory (measured from the factory boot)

| Pool | Size |
|---|---|
| Internal RAM for the heap | 203 + 21 + 32 KiB (plus 7 KiB RTC) |
| PSRAM added to the heap | 8,192 KiB |
| Reserved internal for DMA | 32 KiB |

Factory partition table: `nvs` 24 KB at 0x9000, `phy_init` at 0xf000, and a
single **6 MB factory app** at 0x10000. The remaining ~9.9 MB of flash is
unused by the demo: room for a data partition (NVS, like Game Matcher's
`gmstate`) and OTA slots.

## Running this repo's apps on it

Nothing here has been tried yet; this is what would change, in order of effort:

1. **Plain miniframework app (no screen):** target `xtensa-esp32s3-espidf`
   instead of the C6's RISC-V target, with PSRAM enabled in `sdkconfig`
   (`CONFIG_SPIRAM=y`, **octal** mode, `CONFIG_SPIRAM_MODE_OCT=y`). Rust on
   Xtensa needs the `esp` toolchain (`espup`), which the S3 N16R8 work already
   uses. HTTPS and HTTP/2 get a lot more comfortable with PSRAM.
2. **Game Matcher on this board:** the screen code draws in strips and is
   panel-agnostic, but the display driver (`components/display`) is ST7789-
   specific. It needs a JD9853 init sequence (Waveshare's demo and the
   CircuitPython port both have one), a 34-pixel column offset (same as the
   C6), and touch input to replace BOOT. With PSRAM, a full framebuffer and
   smoother transitions become an option.
3. **Battery:** readable through an ADC (the factory demo calibrates it at
   boot). The exact pin isn't documented on the pages consulted. Read it from
   the schematic before using it.

## Pin reference (for whoever writes the drivers)

You don't need these to use the board. They're here so a future driver
doesn't have to rediscover them. Sources: the factory boot log (what the
demo actually configured) and the CircuitPython port, which says it follows
Waveshare's wiki.

| Function | GPIO | Confidence |
|---|---|---|
| LCD SPI clock / MOSI | 38 / 39 | CircuitPython |
| LCD CS / DC / reset | 21 / 45 / 40 | DC + reset match the factory log |
| LCD backlight (PWM, active high) | 46 | CircuitPython |
| Touch I2C SCL / SDA | 41 / 42 | both agree |
| Touch interrupt / reset | 47 / 48 | **conflict:** the factory log sets 47 as an interrupt input and 48 as an output; CircuitPython names them the other way round. Trust the factory firmware until checked |
| SD (SDMMC 4-bit) CLK / CMD / D0–D3 | 16 / 15 / 17, 18, 13, 14 | both agree |
| BOOT button | 0 | factory log |
| UART TX / RX | 43 / 44 | CircuitPython |
| Octal PSRAM | 33–37 | never use these |

## Recovery: the factory backup

Before anything was written, the whole 16 MB flash was read off the board
(read-only, 3 min 39 s at 921,600 baud):

```text
factory-backups/esp32s3-touch-lcd-1.47-28848ab170e4.bin   (gitignored)
16,777,216 bytes, SHA-256 06822bf927ecfb4f319183fcc5c6995d866b804c7d1aa079429b44e67fbd8b64
```

To put the factory demo back exactly as it shipped:

```sh
python -m esptool --chip esp32s3 --port COM5 -b 921600 write_flash 0 \
  factory-backups/esp32s3-touch-lcd-1.47-28848ab170e4.bin
```

If the board ever stops showing up on USB (a bad app can take the native USB
port down with it), hold **BOOT**, tap **RESET**, release BOOT: it comes up in
the ROM bootloader, which always answers.

## Quick reference

```sh
# identify (read-only)
python -m esptool --port COM5 flash_id
# see what it's doing: reset and read ~8 s of boot log (115200)
# (scratch script used: pulse RTS, then read the port)
# the C6 and this board are both 303A:1001: tell them apart by MAC
#   C6 Game Matcher  ac:eb:e6:1e:13:40
#   this board       28:84:85:b1:70:e4
```

## Manufacturer and community references

- [Waveshare product page](https://www.waveshare.com/ESP32-S3-Touch-LCD-1.47.htm): components, ETA6098 charger, 22-pin header, -M variant (header pre-soldered)
- [Waveshare docs](https://docs.waveshare.com/ESP32-S3-Touch-LCD-1.47): specs, JD9853 + AXS5106L; schematic under Resources
- [CircuitPython board port](https://github.com/adafruit/circuitpython/pull/10689): pin map and JD9853 init sequence
- [LovyanGFX request](https://github.com/lovyan03/LovyanGFX/issues/746): community pins (its "RST 47" disagrees with the factory firmware; don't trust it)
