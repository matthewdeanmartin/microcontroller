"""Convert the S2 ELF and collect boot files without touching a board."""
from pathlib import Path
import shutil
import subprocess
import sys

elf = Path(sys.argv[1]).resolve()
out = Path('firmware')
out.mkdir(exist_ok=True)
image = out / 'async-worker-s2.bin'
subprocess.run([sys.executable, '-m', 'esptool', '--chip', 'esp32s2',
                'elf2image', '--flash_mode', 'dio', '--flash_size', '4MB',
                '--output', str(image), str(elf)], check=True)
if not 0 < image.stat().st_size <= 0x180000:
    raise SystemExit('Application exceeds the 1536 KiB factory partition')
builds = list((elf.parent / 'build').glob('esp-idf-sys-*/out/build'))
builds = [p for p in builds if (p / 'bootloader/bootloader.bin').exists()]
if len(builds) != 1:
    raise SystemExit('Expected exactly one ESP-IDF build tree; use a fresh CARGO_TARGET_DIR')
for source, name in [('bootloader/bootloader.bin', 'bootloader.bin'),
                     ('partition_table/partition-table.bin', 'partition-table.bin')]:
    shutil.copyfile(builds[0] / source, out / name)
print(f'S2 application: {image.stat().st_size} / {0x180000} bytes; images in {out}')
