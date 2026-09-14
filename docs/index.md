# Microcontroller Experiments

Notes and working code from messing about with microcontrollers.

## Boards

| Board | Chip | Notes |
|-------|------|-------|
| DiGiYes ESP32-S2 Mini V1.0.0 | ESP32-S2FN4R2 | 4MB flash, 2MB PSRAM, native USB, 2.4GHz WiFi |

## Where to start

[Basic Setup](basic_setup/index.md) walks through getting an ESP32-S2 from
"just unboxed" to "serving a web page on your WiFi", on Windows, including the
failures you will probably hit along the way. It uses **C** and Espressif's
ESP-IDF toolchain.

[MicroPython](micropython/index.md) does the same job in **Python**, with a
local preview so you can see your page in a browser before it touches the
board, and a three-second deploy instead of a rebuild-and-reflash cycle.

[Secret Messages](secret_messages/index.md) is the first one that does
something useful: a pastebin for a house, where each message is encrypted so
that only the people it was written for can read it. It reuses the MicroPython
workflow wholesale and spends the saved effort on the interesting part.

## Which one?

| | [C / ESP-IDF](basic_setup/index.md) | [MicroPython](micropython/index.md) |
|---|---|---|
| Best for | speed, precise timing, full hardware access | iterating quickly, web things, learning |
| Edit → running | ~1 minute | ~3 seconds |
| Preview without the board | no | yes |

Both are on this repo and either can be flashed over the other in about a
minute, so the choice is not permanent. See
[Trade-offs](micropython/trade_offs.md).
