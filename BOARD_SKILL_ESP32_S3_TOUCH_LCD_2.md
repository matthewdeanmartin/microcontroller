# Board Skill — Waveshare ESP32-S3-Touch-LCD-2

Working notes for the **2-inch touch** Waveshare board (sold without the
camera), plugged in and checked October 17, 2026. Nothing has been flashed to
it: it still runs Waveshare's factory diagnostics app, and a full backup of the
flash is saved (see [Recovery](#recovery-the-factory-backup)). Companion
references: [S3 Touch LCD 1.47](BOARD_SKILL_ESP32_S3_TOUCH_LCD_1_47.md) (same
chip, smaller screen), [C6 LCD 1.47](BOARD_SKILL_ESP32_C6_LCD_1_47.md) (Game
Matcher's board), [S3 N16R8](BOARD_SKILL_ESP32_S3_N16R8.md),
[S2 Mini](BOARD_SKILL_ESP32_S2_MINI.md).

Marked **measured** = probed on this unit (esptool, its boot log, its flash).
The rest is from Waveshare's product page, linked at the end.

## The short version

Same brains as the 1.47" S3 Touch (dual-core 240 MHz S3, 8 MB PSRAM, 16 MB
flash) with a **bigger, squarer screen** and more sensors:

- **2" IPS, 240 × 320**: 40% more pixels than the 1.47" boards (76,800 vs
  55,040) and a far less letterbox shape. It's the first board here where a
  small web-style layout (a header, a list, two buttons) fits comfortably.
- **Touch** (CST816D, single point), so taps rather than the BOOT button.
- **Motion sensor** (QMI8658: accelerometer + gyroscope): it knows which way
  up it is and when it's picked up or shaken.
- **Battery**: a LiPo socket (MX1.25, 3.7 V) with charging on board. No
  battery came with it.
- **Camera socket** (OV2640 / OV5640): empty on this unit, as ordered.
- No RGB LED, speaker, microphone or real-time clock.

## Identity (measured)

```text
Chip:          ESP32-S3 (QFN56), revision v0.2  — sold as ESP32-S3R8
Features:      Wi-Fi, BLE, Embedded PSRAM 8MB (AP_3v3)
Flash:         16 MB, quad; device 0x4018
USB mode:      USB-Serial/JTAG (native USB, one Type-C port)
Base MAC:      28:84:85:88:ce:28
Windows port:  COM20 this session; USB 303A:1001
```

Every ESP32-S3/C6 board in the house shows up as the same `303A:1001` USB
device. Tell them apart by MAC (esptool's `flash_id` prints it):

| MAC | Board | Running |
|---|---|---|
| `ac:eb:e6:1e:13:40` | C6 LCD 1.47 | Game Matcher |
| `28:84:85:b1:70:e4` | S3 Touch LCD 1.47 | Waveshare demo |
| `28:84:85:88:ce:28` | **S3 Touch LCD 2 (this one)** | Waveshare diagnostics |
| `ac:a7:04:2c:38:b8` | S3 (mastomini-bots) | mastomini-bots |

Probing a board with esptool **resets it**. That's harmless for a board
running our firmware (mastomini-bots rebooted and was serving again within
5 s during this session), but it does interrupt it.

## Compared with its siblings

| | **S3 Touch LCD 2** | S3 Touch LCD 1.47 | C6 LCD 1.47 |
|---|---|---|---|
| Screen | **2", 240 × 320, IPS, ST7789** | 1.47", 172 × 320, JD9853 | 1.47", 172 × 320, ST7789 |
| Touch | CST816D | AXS5106L | none |
| CPU | 2 × 240 MHz | 2 × 240 MHz | 1 × 160 MHz |
| RAM | ~320 KB + **8 MB PSRAM** | ~260 KB + 8 MB PSRAM | ~300 KB |
| Flash | 16 MB | 16 MB | 8 MB |
| Motion sensor | **QMI8658** | none | none |
| Battery | LiPo + charger | LiPo + charger | no |
| Camera socket | **yes** | no | no |
| RGB LED | no | no | yes |
| Wi-Fi | 4 (2.4 GHz) | 4 (2.4 GHz) | 6 (2.4 GHz) |

The display controller is the same family (ST7789) as the C6's, so Game
Matcher's existing panel driver is the closest starting point of any board
here. It needs the new size (240 wide, no 34-pixel offset) and pins, not a new
controller.

## What the factory app does (measured)

It's an **Arduino** sketch (Arduino-ESP32 3.1.1 on ESP-IDF 5.3.2, built
January 6, 2025): a diagnostics screen. Its built-in text shows what it
reports:

- **Chip info:** model, cores, CPU/XTAL frequency, embedded flash/PSRAM,
  Arduino and ESP-IDF versions, compile time.
- **Memory:** free / minimum free / largest block for internal RAM and PSRAM;
  flash chip, page, sector and block sizes.
- **Live readings:** battery voltage, chip temperature, Wi-Fi scan with RSSI,
  the motion sensor ("Error initializing IMU" if it fails).
- **Camera:** probes for one. On this unit it logs
  `Camera probe failed … ESP_ERR_NOT_FOUND`, which is expected: no camera
  was ordered.

At boot it prints only three lines to USB: `Find CST816!` (touch found), then
the two camera errors. Everything else goes to the screen.

## Memory and flash layout (measured)

The factory partition table is Arduino's standard 16 MB layout with OTA:

| Partition | Offset | Size | Contents |
|---|---|---|---|
| nvs | 0x9000 | 20 KB | settings |
| otadata | 0xe000 | 8 KB | boots `app0` |
| app0 | 0x10000 | 3 MB | the diagnostics app |
| app1 | 0x310000 | 3 MB | empty (OTA slot) |
| ffat | 0x610000 | ~9.9 MB | **empty**: never formatted, all erased |
| coredump | 0xff0000 | 64 KB | crash dumps |

So the 3 MB `app0` is the entire demo: no images or files elsewhere. A
3 MB app slot is already 20% bigger than Game Matcher's 2.5 MB one on the C6,
and our own layout could make it larger.

## Running this repo's apps on it

Nothing has been tried yet. Same path as the 1.47" S3 Touch: the Xtensa
target (`xtensa-esp32s3-espidf`, `espup` toolchain) with octal PSRAM turned on
in `sdkconfig`. Then, for something like Game Matcher:

1. Display: reuse the ST7789 driver with 240 × 320 and this board's pins.
2. Touch: CST816D over I2C. A tap = "next card"; a long press = pin it.
3. Motion sensor: pick-up-to-wake, or rotate the layout when it's turned.
4. Battery: show the charge on screen; run unplugged for a game night on the table.

## Recovery: the factory backup

The whole 16 MB flash was read off before anything else (read-only,
3 min 29 s):

```text
factory-backups/esp32s3-touch-lcd-2-288488ce28.bin   (gitignored)
16,777,216 bytes, SHA-256 18ea27787cf75099633258738d60b671a09f5b81684235c05942e3fbcf4cc24a
```

To put the diagnostics app back exactly as it shipped:

```sh
python -m esptool --chip esp32s3 --port COM20 -b 921600 write_flash 0 \
  factory-backups/esp32s3-touch-lcd-2-288488ce28.bin
```

If USB ever disappears after flashing something broken: hold **BOOT**, tap
**RESET**, release BOOT. The ROM bootloader always answers.

## Pin reference

Not needed to use the board; nothing is being soldered. Waveshare's pinout is
an image on its wiki; when a driver gets written, take pins from Waveshare's
demo code or the schematic (Resources page), not from community posts. The
1.47" write-up found the community pins disagreeing with the factory
firmware.

## Manufacturer references

- [Waveshare product page](https://www.waveshare.com/ESP32-S3-Touch-LCD-2.htm): ST7789T3, CST816D, QMI8658, MX1.25 battery header, OV2640/OV5640 camera socket, `-C` variant ships with the OV5640
- [Waveshare wiki](https://docs.waveshare.com/ESP32-S3-Touch-LCD-2): pinout image, schematic and demos under Resources
