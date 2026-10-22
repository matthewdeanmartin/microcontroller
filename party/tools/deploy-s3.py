"""Guarded USB deployment of party to one identified ESP32-S3 N16R8.
Run from Git Bash (`make flash` / `make update`).

--replace  FIRST INSTALL / REPAVE: erases the whole board (including any
           previous app's data), then writes bootloader, table and app.
--update   App-only update: refuses unless the board already has exactly
           this partition table, so NVS and the partydata partition survive.

Both need --expected-mac and verify the chip, 16 MB flash and base MAC before
writing anything. There is no default MAC: party has no board assigned yet,
and the household's other S3s hold mastomini, the bots and NanaCoin.
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EMBUILD = ROOT / "api/.embuild"
ESP_PYTHON = os.environ.get(
    "PARTY_ESP_PYTHON",
    "C:/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe" if os.name == "nt" else sys.executable,
)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", required=True)
    parser.add_argument("--expected-mac", required=True, help="the board's base MAC, from DEPLOY.md")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--replace", action="store_true")
    mode.add_argument("--update", action="store_true")
    args = parser.parse_args()

    manifest = json.loads((EMBUILD / "deployment-images.json").read_text())
    if manifest["chip"] != "esp32s3" or manifest["flash_bytes"] != 0x1000000:
        raise SystemExit("Refusing: manifest is not for a 16 MB ESP32-S3")
    images = manifest["images"]
    if [i["address"] for i in images] != [0x0, 0x8000, 0x20000]:
        raise SystemExit("Refusing: unexpected image offsets")
    for image, limit in zip(images, [0x8000, 0x1000, 0x400000]):
        data = Path(image["path"]).read_bytes()
        if len(data) != image["size"] or len(data) > limit or hashlib.sha256(data).hexdigest() != image["sha256"]:
            raise SystemExit("Refusing: an image changed since the build or exceeds its partition; rebuild first")
    print(f"Features: {manifest.get('features', '?')}")

    base = [ESP_PYTHON, "-m", "esptool", "--chip", "esp32s3", "--port", args.port]
    result = subprocess.run(base + ["flash_id"], check=True, capture_output=True, text=True)
    print(result.stdout, end="")
    # Newer esptool prints an extended "MAC:" line before "BASE MAC:".
    found = re.search(r"^BASE MAC:\s*([0-9a-f:]+)", result.stdout, re.M | re.I) or re.search(
        r"^MAC:\s*([0-9a-f:]+)", result.stdout, re.M | re.I
    )
    if not found or found.group(1).lower() != args.expected_mac.lower() or "Detected flash size: 16MB" not in result.stdout:
        raise SystemExit("Board MAC / 16 MB flash does not match the selected unit; nothing written")

    flash = []
    if args.update:
        with tempfile.TemporaryDirectory() as tmp:
            current = Path(tmp) / "table.bin"
            subprocess.run(base + ["read_flash", "0x8000", "0x1000", str(current)], check=True)
            ours = Path(images[1]["path"]).read_bytes()
            if current.read_bytes()[: len(ours)] != ours:
                raise SystemExit("The board's partition table is not party's; use --replace (erases it). Nothing written")
        flash += [hex(images[2]["address"]), images[2]["path"]]
        print("Updating the app only; NVS and partydata are kept.", flush=True)
    else:
        for image in images:
            flash += [hex(image["address"]), image["path"]]
        print("Replacing this board: erase all flash, then install party.", flush=True)
        subprocess.run(base + ["erase_flash"], check=True)
    subprocess.run(
        base + ["--baud", "460800", "write_flash", "--flash_size", "16MB", "--flash_mode", "dio", "--flash_freq", "80m"] + flash,
        check=True,
    )
    print("Flash verified and board reset. Check /api/v1/status before declaring the deployment done.")


if __name__ == "__main__":
    main()
