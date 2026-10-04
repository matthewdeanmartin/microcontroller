# Screen Info

System dashboard for the Waveshare **ESP32-C6-LCD-1.47**, using native
ESP-IDF 5.5.3. This attached board was probed on COM17: **ESP32-C6FH8,
revision 0.2, 8 MB embedded flash**, 40 MHz crystal, USB Serial/JTAG.
Waveshare's original documentation describes a 4 MB FH4 version; this project
targets the attached 8 MB board. It does not target the Touch model.

## Live display and website

The screen updates approximately once a second with Wi-Fi connection/IP,
RSSI in dBm (less negative is stronger), channel, chip temperature, uptime,
free/minimum heap, join attempt count and the last Wi-Fi disconnect reason.
Chip temperature reflects silicon temperature, not ambient temperature.

Press **BOOT** while the application is running to switch to memory/storage:
free/total heap, largest available allocation, flash capacity, internal
filesystem free/total capacity, optional microSD free space, and partition
sizes. Press again to return. The non-Touch model has no touch sensor or IMU;
tapping its glass cannot trigger a page change. BOOT is debounced and only
changes views once per press. Do not hold BOOT across a reset for normal use.

The screen shows the DHCP IP. Open `http://<that-ip>/` for the LAN dashboard.
Its **Memory & storage / Live screen** button also switches the physical LCD,
so everyday use requires no BOOT button presses. `POST /api/view/memory` and
`POST /api/view/live` select the view; `/api/status` includes `screen_view`.
`GET /api/status` exposes live JSON, including every partition's label, type,
subtype, address and size. Wi-Fi retries indefinitely, waiting five seconds
after a failed attempt. Each active attempt gets up to **60 seconds for
association and DHCP**; a stuck attempt is disconnected before retrying.
Five seconds is the retry delay, not the connection timeout. The screen
distinguishes joining from waiting for DHCP. JSON includes attempts, elapsed
attempt time, timeout count and numeric/text disconnect reasons.
The HTTP server starts before joining succeeds so the display stays usable
when the router is unavailable. `waveshare-c6` is the DHCP hostname; there is
no mDNS `.local` responder in this application.

Memory, uptime, link status and storage counters are system telemetry.
The built-in temperature sensor and radio RSSI are the available sensor
readings on this model. There is no humidity, light, room temperature or
battery measurement reported by this firmware.

## Build and deploy on this Windows workspace

```powershell
cd C:\github\microcontroller\screen_info
.\build.ps1
.\deploy.ps1 -Port COM17
```

The helpers activate the repository's existing `../idf-env.ps1`. Wi-Fi
credentials come from `SCREEN_INFO_WIFI_SSID` / `SCREEN_INFO_WIFI_PASSWORD`,
or an ignored `config.py` here, then the existing `hello_wifi_s3_py/config.py`
or `hello_wifi_py/config.py`. Credentials are parsed as literal assignments,
not executed. The generated `wifi_config.h` is ignored and values are never
printed. Wi-Fi credentials are embedded in the local firmware binary.

For the serial POST log/IP:

```powershell
. ..\idf-env.ps1
idf.py -p COM17 monitor
```

Exit the monitor with Ctrl+]. Close it before flashing. Deploy without `-Port`
selects a port only when exactly one Espressif USB device is connected.
Esptool verifies the connected chip is a C6 before writing.

## POST / RGB lights

Based on `../nanacoin/nanacoin_rs/src/bin/esp32/status_led.rs` and
`src/board_status.rs`: dim white startup marker for 1.5 seconds, a gap, then
white reset-class flashes (1 power-on, 2 software/external/USB reset, 3 panic,
4 watchdog, 5 brownout, 6 other). Initializing blinks purple. After POST,
amber blinking means joining/disconnected; a brief green pulse every two
seconds means connected.

A fatal startup error repeatedly flashes the current stage number in red:
200 ms on / 200 ms off per flash, followed by a 1.6 second gap.

| Flashes | Stage |
|---|---|
| 1 | NVS |
| 2 | LCD / shared SPI bus |
| 3 | Temperature sensor |
| 4 | Flash identification / storage setup |
| 5 | Wi-Fi driver, configuration, start |
| 6 | HTTP server |
| 7 | Monitor task / runtime display |

POST confirms driver initialization; the LCD and WS2812 LED have no physical
acknowledgement, so POST cannot prove their pixels visibly work. Optional SD
absence or an unavailable filesystem logs a warning and allows the dashboard
to run. Failed NVS is reported without automatically erasing it.

## Storage and wiring

8 MB layout: 24 KiB NVS, 4 KiB PHY, 2 MiB factory application, 6080 KiB SPIFFS,
plus the bootloader/partition-table area. Partition size is reserved capacity,
not the size of a file or the number of unused application bytes. Filesystem
free space comes from SPIFFS; SD free space comes from FATFS. No telemetry is
persisted and SD is never automatically formatted. Insert a FAT32 microSD
before boot; hot insertion/removal is not supported. A missing/unmountable
card is shown as unavailable rather than a mounted zero-capacity drive.

Flashing writes an initial SPIFFS image containing `storage/README.txt`; a full
deploy replaces this app's internal filesystem image. Runtime mount does not
format anything. The original complete 8 MB factory image is backed up locally
as ignored `factory-backup.bin` before deployment. To restore that image:

```powershell
. ..\idf-env.ps1
python -m esptool --chip esp32c6 --port COM17 write_flash 0 factory-backup.bin
```

LCD: MOSI GPIO6, SCLK GPIO7, CS GPIO14, DC GPIO15, reset GPIO21,
backlight GPIO22. RGB WS2812: GPIO8. BOOT: GPIO9. SD: MISO GPIO5,
CS GPIO4, MOSI/SCLK shared with LCD. Screen backlight is 40% to respect
Waveshare's recommendation of at most 50%. Rendering uses one 8256-byte DMA
strip, rather than a 110080-byte full-screen buffer, and waits for each transfer
before reusing the pixels.

Hardware sources:

- [Waveshare non-Touch board, pinout and brightness guidance](https://docs.waveshare.com/ESP32-C6-LCD-1.47)
- [Manufacturer schematic and demos](https://docs.waveshare.com/ESP32-C6-LCD-1.47/Resources-And-Documents)
- [Separate Touch board: different LCD, wiring and IMU](https://docs.waveshare.com/ESP32-C6-Touch-LCD-1.47)

## Bring-up, September 30, 2026

Built with the installed ESP-IDF 5.5.3 and flashed COM17; esptool verified the
written image hashes. All seven POST stages completed. The board obtained
`192.168.1.163`, and both `/` and `/api/status` returned successfully. Live
telemetry reported 8,388,608 flash bytes, a mounted SPIFFS filesystem with
5,715,521 usable bytes, and an absent/unavailable SD card. The user confirmed
that the physical LCD displays live readings and BOOT switches views. Physical
LED patterns have not yet been independently confirmed by the user.
Credentials, factory backup, build output and console logs remain
ignored. `serial.log`, `flash.log` and `build.log` contain the local bring-up
records; the original full-flash backup is `factory-backup.bin`.

The updated firmware joined in one attempt (three seconds). Both web view
controls were exercised: status changed to `memory`, then back to `live`.
The device remained connected and reported its 60-second attempt budget.
Forced failure/slow DHCP recovery remains untested.
