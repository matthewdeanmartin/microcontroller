#!/usr/bin/env bash
# Usage: bash tools/build-s3.sh [cargo feature,...]
#
# Builds the party firmware for an ESP32-S3 N16R8 (xtensa-esp32s3-espidf).
# Git Bash, Windows ESP-IDF install at C:\Espressif (elsewhere, source
# ESP-IDF's and espup's export scripts first). Writes the app image,
# bootloader and partition table plus a manifest to api/.embuild/.
# Compile only: never opens a serial port.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
app=$root/api
features=esp32${1:+,$1}
bin=party-s3
target=xtensa-esp32s3-espidf
export MCU=esp32s3

[[ -f $app/.embuild/web/assets.rs ]] || { echo "No web bundle: run 'make web' first." >&2; exit 1; }
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }

# esp-idf-sys generates its CMake project elsewhere; a relative partition
# table path would resolve there. Point it at this app's file.
mkdir -p "$app/.embuild"
sed "s#\"partitions.csv\"#\"$(native "$app/partitions.csv")\"#" "$app/sdkconfig.defaults" >"$app/.embuild/sdkconfig.defaults"
export ESP_IDF_SDKCONFIG_DEFAULTS
ESP_IDF_SDKCONFIG_DEFAULTS=$(native "$app/.embuild/sdkconfig.defaults")

# With a shared/short target directory esp-idf-sys cannot find the root
# crate's [package.metadata.esp-idf-sys] (mDNS would be silently dropped).
export CARGO_WORKSPACE_DIR ESP_IDF_SYS_ROOT_CRATE
CARGO_WORKSPACE_DIR=$(native "$app")
ESP_IDF_SYS_ROOT_CRATE=party

# A short build fingerprint the board reports in /api/v1/status.
export PARTY_BUILD
PARTY_BUILD=$(cd "$root" && git rev-parse --short HEAD 2>/dev/null || echo dev)$(cd "$root" && git diff --quiet -- . 2>/dev/null && git diff --cached --quiet -- . 2>/dev/null && [[ -z $(git ls-files --others --exclude-standard -- . 2>/dev/null) ]] || echo +)

idf=/c/Espressif/frameworks/esp-idf-v5.5.3
if [[ -d $idf ]]; then
  # esp-idf-sys rejects long Windows output paths; keep the target dir short.
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-C:/tmp/pts3}"
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

echo "Features: $features"
cd "$app"
"${cargo_cmd[@]}" build --release --no-default-features --features "$features" \
  --bin "$bin" --target "$target" -Z build-std=std,panic_abort

out="${CARGO_TARGET_DIR:-$app/target}/$target/release"
image=$app/.embuild/$bin.bin
"$python" -m esptool --chip esp32s3 elf2image --flash_mode dio --flash_freq 80m --flash_size 16MB \
  --output "$image" "$out/$bin"
build_dir=$(ls -td "$out"/build/esp-idf-sys-*/out/build 2>/dev/null | head -1)
cp "$build_dir/bootloader/bootloader.bin" "$app/.embuild/bootloader.bin"
cp "$build_dir/partition_table/partition-table.bin" "$app/.embuild/partition-table.bin"
"$python" "$root/tools/manifest.py" "$app/.embuild" "$features"
