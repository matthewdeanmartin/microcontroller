#!/usr/bin/env bash
# Usage: bash tools/build-c6.sh [cargo feature,...]
#
# Builds the Game Matcher firmware for the Waveshare ESP32-C6-LCD-1.47
# (riscv32imac-esp-espidf). Git Bash, Windows ESP-IDF install at
# C:\Espressif (elsewhere, source ESP-IDF's export script first). Writes
# the app image, bootloader and partition table plus a manifest to
# api/.embuild/. Compile only: never opens a serial port.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
app=$root/api
features=esp32${1:+,$1}
bin=gamematcher-c6
target=riscv32imac-esp-espidf
export MCU=esp32c6

[[ -f $app/.embuild/web/assets.rs ]] || { echo "No web bundle: run 'make web' first." >&2; exit 1; }
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }

# esp-idf-sys generates its CMake project elsewhere; a relative partition
# table path would resolve there. Point it at this app's file.
mkdir -p "$app/.embuild"
sed "s#\"partitions.csv\"#\"$(native "$app/partitions.csv")\"#" "$app/sdkconfig.defaults" >"$app/.embuild/sdkconfig.defaults"
# esp-idf-sys doesn't watch extra components' C sources; a digest of them in
# the watched defaults makes a changed display driver rebuild.
echo "# components/display digest: $(cat "$app"/components/display/*.c "$app"/components/display/include/*.h | sha256sum | cut -c1-16)" >>"$app/.embuild/sdkconfig.defaults"
export ESP_IDF_SDKCONFIG_DEFAULTS
ESP_IDF_SDKCONFIG_DEFAULTS=$(native "$app/.embuild/sdkconfig.defaults")

# With a shared/short target directory esp-idf-sys cannot find the root
# crate's [package.metadata.esp-idf-sys] (mDNS would be silently dropped).
export CARGO_WORKSPACE_DIR ESP_IDF_SYS_ROOT_CRATE
CARGO_WORKSPACE_DIR=$(native "$app")
ESP_IDF_SYS_ROOT_CRATE=gamematcher

# A short build fingerprint the board reports in /api/v1/status.
export GAMEMATCHER_BUILD
GAMEMATCHER_BUILD=$(cd "$root" && git rev-parse --short HEAD 2>/dev/null || echo dev)$(cd "$root" && git diff --quiet -- . 2>/dev/null && git diff --cached --quiet -- . 2>/dev/null && [[ -z $(git ls-files --others --exclude-standard -- . 2>/dev/null) ]] || echo +)

idf=/c/Espressif/frameworks/esp-idf-v5.5.3
if [[ -d $idf ]]; then
  # esp-idf-sys rejects long Windows output paths; keep the target dir short.
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-C:/tmp/gmc6}"
  export IDF_PATH="C:/Espressif/frameworks/esp-idf-v5.5.3"
  export IDF_TOOLS_PATH="C:/Espressif"
  export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
  export IDF_PYTHON_ENV_PATH="C:/Espressif/python_env/idf5.5_py3.11_env"
  export ESP_ROM_ELF_DIR="C:/Espressif/tools/esp-rom-elfs/20241011"
  export LIBCLANG_PATH="$(cygpath -m "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin/libclang.dll"
  export PATH="$idf/tools:/c/Espressif/tools/riscv32-esp-elf/esp-14.2.0_20251107/riscv32-esp-elf/bin:/c/Espressif/tools/cmake/3.30.2/bin:/c/Espressif/tools/ninja/1.12.1:/c/Espressif/python_env/idf5.5_py3.11_env/Scripts:$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin:$PATH"
  python=/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe
else
  python=python
fi

echo "Features: $features"
cd "$app"
cargo +esp build --release --no-default-features --features "$features" \
  --bin "$bin" --target "$target" -Z build-std=std,panic_abort

out="${CARGO_TARGET_DIR:-$app/target}/$target/release"
image=$app/.embuild/$bin.bin
"$python" -m esptool --chip esp32c6 elf2image --flash_mode dio --flash_freq 80m --flash_size 8MB \
  --output "$image" "$out/$bin"
build_dir=$(ls -td "$out"/build/esp-idf-sys-*/out/build 2>/dev/null | head -1)
cp "$build_dir/bootloader/bootloader.bin" "$app/.embuild/bootloader.bin"
cp "$build_dir/partition_table/partition-table.bin" "$app/.embuild/partition-table.bin"
"$python" "$root/tools/manifest.py" "$app/.embuild" "$features"
