#!/usr/bin/env bash
# Usage: bash tools/flash.sh <app-dir> <bin-name> <PORT> --confirm-mac MAC [--replace-nanacoin]
#
# Flashes an ESP32-S2 Mini with an app built by tools/build-firmware.sh:
# bootloader (0x1000), partition table (0x8000), app (0x10000).
#
# Before writing anything it:
#   1. connects with --chip esp32s2 (any other chip is refused),
#   2. requires the board's MAC to match --confirm-mac, and
#   3. parses the existing partition table and requires the built layout.
# Empty/unknown layouts need a separately reviewed provisioning/migration.
# --replace-nanacoin never bypasses layout or identity checks.
#
# The S2 Mini has native USB only: put it in download mode first
# (hold BOOT, tap RST, release BOOT), and tap RST again after flashing.
set -euo pipefail
[[ $# -ge 5 ]] || { echo 'Usage: bash tools/flash.sh <app-dir> <bin-name> <PORT> --confirm-mac MAC [--replace-nanacoin]' >&2; exit 2; }
app=$(cd "$1" && pwd)
bin=$2
port=$3
shift 3
confirm_mac=
replace=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --confirm-mac) [[ $# -ge 2 && -z $confirm_mac ]] || exit 2; confirm_mac=${2,,}; shift 2 ;;
    --replace-nanacoin) replace=(--replace-nanacoin); shift ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ $confirm_mac =~ ^([0-9a-f]{2}:){5}[0-9a-f]{2}$ ]] || { echo 'A valid --confirm-mac is required.' >&2; exit 2; }
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
out="${CARGO_TARGET_DIR:-C:/tmp/mfw-s2}/xtensa-esp32s2-espidf/release"
python=python
[[ -x /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe ]] && python=/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe
esptool() { "$python" -m esptool --chip esp32s2 --port "$port" --before no_reset "$@"; }
# The S2's ROM USB loader drops off the bus for a moment after every esptool
# session; wait until the port opens again before the next step.
wait_port() {
  for _ in $(seq 1 30); do
    "$python" -c "import serial,sys; serial.Serial('$port').close()" 2>/dev/null && return 0
    sleep 0.5
  done
  echo "$port did not come back; is the board still in download mode?" >&2
  exit 1
}

for f in bootloader.bin partition-table.bin "$bin.bin"; do
  [[ -f $out/$f ]] || { echo "Missing $out/$f: run 'make firmware' first." >&2; exit 1; }
done

echo "Connecting to $port (the S2 must be in download mode: hold BOOT, tap RST, release BOOT)..."
probe=$(esptool --after no_reset read_mac 2>&1) || { echo "$probe" >&2; exit 1; }
mac=$(sed -n 's/^MAC: //p' <<<"$probe" | head -1)
[[ -n $mac ]] || {
  tail -3 <<<"$probe" >&2
  echo "No ESP32-S2 answered on $port. Put it in download mode again (hold BOOT, tap RST, release BOOT) and retry." >&2
  exit 1
}
echo "Board MAC: $mac"
mac=${mac//$'\r'/}
[[ ${mac,,} == "$confirm_mac" ]] || { echo "MAC mismatch: expected $confirm_mac; nothing was written." >&2; exit 1; }
wait_port

# Windows Python cannot open Git Bash paths like /tmp/...: pass C:/... form.
table=$(mktemp)
trap 'rm -f "$table" "$table.log"' EXIT
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }
esptool --after no_reset read_flash 0x8000 0xC00 "$(native "$table")" >"$table.log" 2>&1 || {
  cat "$table.log" >&2
  echo 'Could not read the partition table; nothing was written.' >&2
  exit 1
}
PYTHONPATH="$(native "$script_dir/boardsafe/src")" uv run --no-project python -m boardsafe.first_install \
  "$(native "$table")" "$(native "$out/partition-table.bin")" "${replace[@]}"
wait_port

# esptool can lose the port after the last write (the chip leaves USB); the
# write counts as done when all three images verified.
written=$(mktemp)
esptool --after no_reset write_flash --flash_mode dio --flash_size 4MB \
  0x1000 "$(native "$out/bootloader.bin")" 0x8000 "$(native "$out/partition-table.bin")" 0x10000 "$(native "$out/$bin.bin")"   2>&1 | tee "$written" || true
verified=$(grep -c 'Hash of data verified' "$written" || true)
rm -f "$written"
[[ $verified -eq 3 ]] || { echo "Only $verified of 3 images verified: flash again." >&2; exit 1; }
echo "Flashed. Tap RST to start it; its log is at http://<board>/api/v1/log.txt."
