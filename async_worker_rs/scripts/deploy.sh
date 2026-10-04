#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${1:-}" ]] || { echo 'Usage: bash scripts/deploy.sh COM_PORT [--provision | --dry-run]' >&2; exit 2; }
port=$1
shift
bash scripts/build-esp32.sh
esp_python="${WORKER_ESPTOOL_PYTHON:-python}"
if [[ -z "${WORKER_ESPTOOL_PYTHON:-}" && -f /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe ]]; then
  esp_python=/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe
fi
"$esp_python" scripts/deploy.py --port "$port" "$@"
