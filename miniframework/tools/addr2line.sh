#!/usr/bin/env bash
# Usage: bash tools/addr2line.sh <bin-name> 0x4008abcd 0x4008ef01 ...
#
# Maps the PCs of a "crash backtrace" line from /api/v1/log to functions and
# source lines, using the ELF of the current firmware build. Only meaningful
# if the board runs exactly that build (compare `build` in /api/v1/sys).
set -euo pipefail
[[ $# -ge 2 ]] || { echo 'Usage: bash tools/addr2line.sh housemetrics-esp32 <pc> [<pc>...]' >&2; exit 2; }
bin=$1
shift
elf="${CARGO_TARGET_DIR:-C:/mfw-s2}/xtensa-esp32s2-espidf/release/$bin"
tool=/c/Espressif/tools/xtensa-esp-elf/esp-14.2.0_20251107/xtensa-esp-elf/bin/xtensa-esp32s2-elf-addr2line.exe
command -v xtensa-esp32s2-elf-addr2line >/dev/null && tool=xtensa-esp32s2-elf-addr2line
[[ -f $elf ]] || { echo "No ELF at $elf: build the firmware first." >&2; exit 1; }
"$tool" -pfiaC -e "$elf" "$@"
