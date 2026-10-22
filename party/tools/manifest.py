"""Checks a built S3 image against the partition table and writes the
deployment manifest that deploy-s3.py verifies. Never opens a port.

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
    "otadata": (1, 0x00, 0xF000, 0x2000),
    "phy_init": (1, 0x01, 0x11000, 0x1000),
    "app0": (0, 0x10, 0x20000, 0x400000),
    "app1": (0, 0x11, 0x420000, 0x400000),
    "partydata": (1, 0x02, 0x820000, 0x780000),
    "coredump": (1, 0x03, 0xFA0000, 0x10000),
}
APP = "app0"
BOOTLOADER_AT = 0x0  # the ESP32-S3's second-stage bootloader offset


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
    app = embuild / "party-s3.bin"
    size = app.stat().st_size
    limit = EXPECTED[APP][3]
    print(f"S3 app image: {size:,} / {limit:,} bytes ({size / limit:.1%} of {APP})")
    if size > limit:
        raise SystemExit(f"Image exceeds {APP}")
    images = []
    for address, name in [(BOOTLOADER_AT, "bootloader.bin"), (0x8000, "partition-table.bin"), (EXPECTED[APP][2], app.name)]:
        data = (embuild / name).read_bytes()
        images.append(
            dict(address=address, path=str((embuild / name).resolve()), size=len(data), sha256=hashlib.sha256(data).hexdigest())
        )
    manifest = dict(
        chip="esp32s3",
        flash_bytes=16 * 1024 * 1024,
        features=features,
        created_at=datetime.now(timezone.utc).isoformat(),
        images=images,
    )
    (embuild / "deployment-images.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Manifest: {embuild / 'deployment-images.json'}")
    print("No board was accessed.")


if __name__ == "__main__":
    main()
