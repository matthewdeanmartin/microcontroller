# Microcontroller Experiments

Notes and working code from messing about with microcontrollers.

## Boards

| Board | Chip | Notes |
|-------|------|-------|
| DiGiYes ESP32-S2 Mini V1.0.0 | ESP32-S2FN4R2 | 4MB flash, 2MB PSRAM, native USB, 2.4GHz WiFi |
| ESP32-S3-N16R8 | ESP32-S3 | 16MB flash, 8MB octal PSRAM, dual-core, WiFi + BLE, UART bridge |

## Where to start

[Basic Setup](basic_setup/index.md) walks through getting an ESP32-S2 from
"just unboxed" to "serving a web page on your WiFi", on Windows, including the
failures you will probably hit along the way. It uses **C** and Espressif's
ESP-IDF toolchain.

[MicroPython](micropython/index.md) does the same job in **Python**, with a
local preview so you can see your page in a browser before it touches the
board, and a three-second deploy instead of a rebuild-and-reflash cycle.

[ESP32-S3](esp32s3/index.md) covers the second board: what differs from the S2,
the two ways to silently brick the flash, and why the dashboard it serves keeps
all its intelligence in the browser.

[Secret Messages](secret_messages/index.md) is the first one that does
something useful: a pastebin for a house, where each message is encrypted so
that only the people it was written for can read it. It reuses the MicroPython
workflow wholesale and spends the saved effort on the interesting part.

## Which one?

[Rust and NanaCoin](rust/index.md) explains the active Rust implementation:
ownership and bounded memory, ESP-IDF and PSRAM, NVS keys and checkpoint
recovery, two-core execution, and the shared Angular diagnostic dashboard.
Examples link to the implementation so you can follow the code while learning.

[TinyGo and NanaCoin](tinygo/index.md) follows a **Go** application onto the
ESP32-S3: a household currency, ledger and marketplace with an Angular client.
It explains firmware development, the bounded web framework, memory management,
concurrency and evidence-driven load testing. Start here if you know application
programming and want to understand what changes on a tiny device.

| | [C / ESP-IDF](basic_setup/index.md) | [MicroPython](micropython/index.md) |
|---|---|---|
| Best for | speed, precise timing, full hardware access | iterating quickly, web things, learning |
| Edit → running | ~1 minute | ~3 seconds |
| Preview without the board | no | yes |

The C and MicroPython examples are in this repo and either can be flashed over the other in about a
minute, so the choice is not permanent. See
[Trade-offs](micropython/trade_offs.md).
