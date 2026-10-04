# Board Skill — Waveshare ESP32-C6-LCD-1.47

Working notes for the attached **non-Touch** 1.47-inch Waveshare board,
identified and deployed September 30, 2026. Companion references:
[S2 Mini](BOARD_SKILL_ESP32_S2_MINI.md) and
[S3 N16R8](BOARD_SKILL_ESP32_S3_N16R8.md).

Hardware probe results and user observations are marked below. Wiring and
nominal features come from the linked manufacturer documentation. The app is
[`screen_info/`](screen_info/README.md), built with ESP-IDF 5.5.3.

## Identity: this unit versus the listing

The user read **ESP32-C6-LCD-1.47**, with no `Touch`, from the board label.
Esptool directly reported:

```text
Chip:          ESP32-C6FH8 (QFN32), revision v0.2
Features:      Wi-Fi 6, BT 5 (LE), IEEE802.15.4,
               Single Core + LP Core, 160MHz, Embedded Flash 8MB
Crystal:       40MHz
USB mode:      USB-Serial/JTAG
Flash ID:      manufacturer 20, device 4017
Flash size:    8MB
Base MAC:      ac:eb:e6:1e:13:40
Windows port:  COM17 during this session
USB VID:PID:   303A:1001
```

**The actual flash is 8 MB.** Waveshare's non-Touch documentation describes
an ESP32-C6FH4 with 4 MB. The attached unit has an FH8; do not reduce it to
4 MB based on that page. Conversely, an FH8 does not by itself prove a board
is the Touch model. Check the label, peripherals and pinout too.

| Property | Value / evidence |
|---|---|
| Main CPU | One RISC-V application core, 160 MHz; confirmed in boot log |
| Low-power CPU | Separate LP RISC-V core; nominal maximum 20 MHz |
| Flash | 8,388,608 bytes, directly probed |
| RAM | Nominal 512 KiB HP SRAM + 16 KiB LP SRAM |
| PSRAM | None on this board |
| Radio | 2.4 GHz Wi-Fi 6 and BLE 5; C6 also reports IEEE802.15.4 capability |
| LCD | 172 × 320 TFT, ST7789 family; manufacturer's pinout/demo |
| Display colors | Panel rated 262K; this app uses 16-bit RGB565 (65,536 colors) |
| Touchscreen | No touch sensor on this model |
| RGB LED | WS2812B, GPIO8; visible in the gap/interlayer between PCB and display |
| Controls | BOOT and RESET; user confirmed BOOT changes app views |
| Storage socket | microSD / TF slot, SPI interface; optional card |
| USB | Native USB Serial/JTAG over Type-C |

## What the socket and the lights are

The little card socket is for a **microSD card**, also called a **TF card**.
It expands file storage. It is not needed for booting, Wi-Fi, the display or
`screen_info`: the firmware and internal filesystem live in the soldered
flash. The factory demo's `0 MB SD card` reading means no usable card was
mounted; it does not mean the board has no internal storage.

Use a FAT32 card with this app and insert it before boot. The firmware never
automatically formats a card. It mounts and reports total/free capacity, and
continues without one. Hot insertion/removal has not been implemented.
No card was mounted during bring-up, so operation with a real card remains
unverified. The published material consulted does not establish a maximum
supported card capacity for this exact unit.

The light visible between the screen and board is the **onboard RGB LED**,
not an extra accessory to connect. Waveshare uses the transparent acrylic
interlayer to spread its light. `screen_info` drives it at low brightness.
Blink meanings depend on the running firmware; the factory demo and this app
do not share the same interpretation.

## Onboard readings and controls

Available live readings include Wi-Fi RSSI, channel/link state and the chip's
internal temperature sensor. Uptime, heap statistics and filesystem space
are system telemetry. Chip temperature is not room temperature.

This non-Touch board has no documented IMU, humidity sensor or light sensor.
Tapping the glass cannot change views. A short **BOOT** press changes views;
the LAN website can also select a screen view, avoiding routine mechanical
button presses. The switch's rated press lifetime has not been established.

The **Touch** product is a separate design: JD9853 display, AXS5106L touch,
QMI8658A motion sensor, battery circuitry and different GPIO wiring. Its
drivers/pinout cannot be substituted into this board's firmware.

## Wiring reference: non-Touch board

| Function | GPIO |
|---|---|
| LCD / SD MOSI | 6 |
| LCD / SD SCLK | 7 |
| LCD CS | 14 |
| LCD DC | 15 |
| LCD reset | 21 |
| LCD backlight | 22 |
| WS2812 RGB data | 8 |
| BOOT | 9 |
| SD MISO | 5 |
| SD CS | 4 |
| USB D− / D+ | 12 / 13 |

LCD and SD share an SPI bus with separate chip selects. Do not assign other
functions to these pins. ST7789 panel coordinates need an X offset of 34.
The Waveshare demo's mirror/color/register settings are applied in
`screen_info/main/display.c`.

Waveshare recommends screen brightness **50% or lower** because prolonged
high brightness can overheat/discolor the panel. This app uses **40% PWM**.

## USB, flashing and recovery

The initial missing-device problem was a **loose USB cable**. Reseating it
made COM17 appear. Automatic bootloader entry and reset worked with esptool;
no physical button presses were required to flash in this session.

Find the port rather than assuming COM17 will always be assigned:

```powershell
python -m serial.tools.list_ports -v
. .\idf-env.ps1
python -m esptool --chip esp32c6 --port COM17 flash_id
```

This installed esptool 4.12 uses underscore subcommands. Build/deploy:

```powershell
cd screen_info
.\build.ps1
.\deploy.ps1 -Port COM17
```

The C6 bootloader is at **0x0**, partition table at **0x8000**, application at
**0x10000**. Use the generated flash arguments; a standalone application
binary is not a complete image for address zero. Target **esp32c6**, not S2/S3.

A complete, ignored 8 MB factory backup was saved before the first write:
`screen_info/factory-backup.bin`. Restore with esptool `write_flash 0` using
that complete backup, as documented in the app README. Full app deployment
also writes the initial SPIFFS image; it replaces files in that partition.

## Memory and storage actually observed

During successful bring-up, `/api/status` reported:

| Resource | Measurement |
|---|---|
| Total allocator heap | 369,500 bytes |
| Free heap, sample | 268,588 bytes; varies with traffic |
| Largest available block, sample | 245,760 bytes |
| SPIFFS usable capacity | 5,715,521 bytes |
| SPIFFS used, initial image | 502 bytes |
| Physical flash | 8,388,608 bytes |
| SD | Not mounted; no card tested |

The app's partition allocation is:

| Partition | Offset | Reserved bytes |
|---|---|---|
| NVS | 0x9000 | 24,576 |
| PHY init | 0xf000 | 4,096 |
| Factory application | 0x10000 | 2,097,152 |
| SPIFFS storage | 0x210000 | 6,225,920 |

Reserved partition bytes, filesystem usable/free bytes and firmware image
size are different quantities. This C6 has ample flash for a small dashboard
but much less working RAM than the S3 with 8 MB PSRAM. Screen rendering uses
an 8,256-byte DMA strip instead of a full 110,080-byte RGB565 framebuffer.

## Wi-Fi joining and POST behavior

The user reports joining sometimes requires **5–10 attempts**, with each
attempt taking more than five seconds. Firmware must keep the screen usable
and retry indefinitely. `screen_info` permits up to **60 seconds per active
association/DHCP attempt** and waits **five seconds between failures**.
It avoids duplicate connect requests while joining, distinguishes association
from waiting for a DHCP address, and records attempts/timeouts/reasons. The
60-second recovery path is implemented; a forced slow/failing join has not
yet been tested. A successful normal join was observed.
The final deployed firmware joined in one attempt, taking three seconds.
The website's memory/live controls were exercised and the device reported
both selected views while remaining connected.

The app initially acquired `192.168.1.163`; this is a DHCP assignment, not a
permanent address. Open the screen's current IP in a browser. Both `/` and
`/api/status` were reached from the computer during bring-up. The DHCP
hostname is `waveshare-c6`; no mDNS `.local` service is implemented.

POST uses NanaCoin's startup/reset marker and numbered failure convention:
white startup/reset flashes, purple initializing, amber joining/disconnected,
brief green pulse when connected. Repeating **red** groups encode the failed
stage, with 200 ms on/off flashes and a 1.6-second group pause:

| Red flashes | Stage |
|---|---|
| 1 | NVS |
| 2 | LCD / SPI |
| 3 | Temperature sensor |
| 4 | Flash / storage setup |
| 5 | Wi-Fi initialization |
| 6 | HTTP server |
| 7 | Monitor / runtime display |

All seven stages completed on the device. The user confirmed readable live
screen data and BOOT view switching. Exact physical LED colors/patterns still
need observation; a driver returning success cannot prove the visible color.

## Manufacturer references

- [Non-Touch board overview, pinout, SD socket and RGB interlayer](https://docs.waveshare.com/ESP32-C6-LCD-1.47)
- [Non-Touch schematics and example firmware](https://docs.waveshare.com/ESP32-C6-LCD-1.47/Resources-And-Documents)
- [Touch model's different peripherals and wiring](https://docs.waveshare.com/ESP32-C6-Touch-LCD-1.47)
