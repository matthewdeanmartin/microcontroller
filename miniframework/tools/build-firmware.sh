#!/usr/bin/env bash
# Usage: bash tools/build-firmware.sh <app-dir> <bin-name> [cargo args...]
#
# Builds an ESP32-S2 firmware image for an app (Git Bash, Windows ESP-IDF
# install at C:\Espressif; elsewhere, source ESP-IDF's and espup's export
# scripts first). Produces <bin>.bin plus the bootloader and partition table
# next to the ELF. Never touches a board.
set -euo pipefail
[[ $# -ge 2 ]] || { echo 'Usage: bash tools/build-firmware.sh <app-dir> <bin-name>' >&2; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
app=$(cd "$1" && pwd)
bin=$2
shift 2
target=xtensa-esp32s2-espidf
export MCU=esp32s2

[[ -f $app/.embuild/web/assets.rs ]] || { echo "No web bundle: run 'make web' first." >&2; exit 1; }

# esp-idf-sys generates a CMake project elsewhere; a relative partition
# table path would resolve there. Point it at this app's file explicitly.
mkdir -p "$app/.embuild"
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }
sed "s#\"partitions.csv\"#\"$(native "$app/partitions.csv")\"#" "$app/sdkconfig.defaults" >"$app/.embuild/sdkconfig.defaults"
export ESP_IDF_SDKCONFIG_DEFAULTS
ESP_IDF_SDKCONFIG_DEFAULTS=$(native "$app/.embuild/sdkconfig.defaults")

# esp-idf-sys reads the app's [package.metadata.esp-idf-sys] (sdkconfig,
# extra components such as mDNS) from the crate it finds here; with a
# shared target directory it cannot guess, so say it.
export CARGO_WORKSPACE_DIR ESP_IDF_SYS_ROOT_CRATE
CARGO_WORKSPACE_DIR=$(native "$app")
ESP_IDF_SYS_ROOT_CRATE=$(grep -m1 '^name = ' "$app/Cargo.toml" | cut -d'"' -f2)

# A short build fingerprint the board reports in /api/v1/sys.
export HOUSEMETRICS_BUILD
HOUSEMETRICS_BUILD=$(cd "$root" && git rev-parse --short HEAD 2>/dev/null || echo dev)$(cd "$root" && git diff --quiet 2>/dev/null || echo +)

idf=/c/Espressif/frameworks/esp-idf-v5.5.3
if [[ -d $idf ]]; then
  # esp-idf-sys rejects long Windows output paths; keep the target dir short.
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-C:/mfw-s2}"
  export IDF_PATH="C:/Espressif/frameworks/esp-idf-v5.5.3"
  export IDF_TOOLS_PATH="C:/Espressif"
  export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
  export IDF_PYTHON_ENV_PATH="C:/Espressif/python_env/idf5.5_py3.11_env"
  export ESP_ROM_ELF_DIR="C:/Espressif/tools/esp-rom-elfs/20241011"
  export PATH="$idf/tools:/c/Espressif/python_env/idf5.5_py3.11_env/Scripts:/c/Espressif/tools/cmake/3.30.2/bin:/c/Espressif/tools/ninja/1.12.1:$PATH"
  export LIBCLANG_PATH="$(cygpath -m "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin/libclang.dll"
  # The IDF's own GCC must win over the one rustup bundles (too new for IDF 5.5).
  export PATH="/c/Espressif/tools/xtensa-esp-elf/esp-14.2.0_20251107/xtensa-esp-elf/bin:$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin:$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp-elf/bin:$PATH"
  # Calling the rustup proxy would put its GCC first again; use the esp
  # toolchain's cargo and rustc directly.
  esp_bin="$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/bin"
  export RUSTC="$(cygpath -m "$esp_bin/rustc.exe")"
  cargo_cmd=("$esp_bin/cargo.exe")
  python=/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe
else
  cargo_cmd=(cargo +esp)
  python=python
fi

cd "$app"
"${cargo_cmd[@]}" build --release --no-default-features --features esp32 \
  --bin "$bin" --target "$target" -Z build-std=std,panic_abort "$@"

out="${CARGO_TARGET_DIR:-$app/target}/$target/release"
"$python" -m esptool --chip esp32s2 elf2image --flash_mode dio --flash_size 4MB \
  --output "$out/$bin.bin" "$out/$bin"
size=$(stat -c %s "$out/$bin.bin")
limit=$(( 0x3E0000 ))
(( size <= limit )) || { echo "Firmware is $size bytes; the app partition holds $limit." >&2; exit 1; }
# The bootloader and partition table esp-idf-sys built for this app.
build_dir=$(ls -td "$out"/build/esp-idf-sys-*/out/build 2>/dev/null | head -1)
cp "$build_dir/bootloader/bootloader.bin" "$out/bootloader.bin"
cp "$build_dir/partition_table/partition-table.bin" "$out/partition-table.bin"
echo "Firmware: $out/$bin.bin ($size of $limit bytes, $(( 100 * size / limit ))%)"
echo "Bootloader and partition table copied next to it. No board was accessed."
