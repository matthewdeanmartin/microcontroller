"""Checks a built C6 image against the partition table and writes the
deployment manifest that deploy-c6.py verifies. Never opens a port.

Usage: python tools/manifest.py <api/.embuild> <features>
"""
import hashlib
import json
import struct
import sys
from datetime import datetime, timezone
from pathlib import Path

# The layout in api/partitions.csv: (type, subtype, offset, size).
EXPECTED = {
    "nvs": (1, 0x02, 0x9000, 0x6000),
    "phy_init": (1, 0x01, 0xF000, 0x1000),
    "factory": (0, 0x00, 0x10000, 0x280000),
    "coredump": (1, 0x03, 0x290000, 0x10000),
    "gmstate": (1, 0x02, 0x2A0000, 0x80000),
}


def partitions(table: bytes) -> dict:
    found = {}
    for offset in range(0, len(table), 32):
        row = table[offset : offset + 32]
        if len(row) < 32 or row[:2] != b"\xaaP":
            break
        _, kind, subtype, address, size, label, _flags = struct.unpack("<HBBII16sI", row)
        found[label.split(b"\0")[0].decode()] = (kind, subtype, address, size)
    return found


def main() -> None:
    embuild = Path(sys.argv[1])
    features = sys.argv[2] if len(sys.argv) > 2 else ""
    table = (embuild / "partition-table.bin").read_bytes()
    if partitions(table) != EXPECTED:
        raise SystemExit(f"Partition table differs from the expected layout: {partitions(table)}")
    app = embuild / "gamematcher-c6.bin"
    size = app.stat().st_size
    limit = EXPECTED["factory"][3]
    print(f"C6 app image: {size:,} / {limit:,} bytes ({size / limit:.1%} of the factory partition)")
    if size > limit:
        raise SystemExit("Image exceeds the factory partition")
    images = []
    for address, name in [(0x0, "bootloader.bin"), (0x8000, "partition-table.bin"), (0x10000, app.name)]:
        data = (embuild / name).read_bytes()
        images.append(
            dict(address=address, path=str((embuild / name).resolve()), size=len(data), sha256=hashlib.sha256(data).hexdigest())
        )
    manifest = dict(
        chip="esp32c6",
        flash_bytes=8 * 1024 * 1024,
        features=features,
        created_at=datetime.now(timezone.utc).isoformat(),
        images=images,
    )
    (embuild / "deployment-images.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Manifest: {embuild / 'deployment-images.json'}")
    print("No board was accessed.")


if __name__ == "__main__":
    main()
