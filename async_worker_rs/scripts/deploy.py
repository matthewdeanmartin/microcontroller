"""App-only upgrades; first provisioning backs up 4 MiB unless explicitly skipped."""
import argparse
import hashlib
from datetime import datetime, timezone
from pathlib import Path
import subprocess
import sys
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--port', required=True)
p.add_argument('--provision', action='store_true', help='Back up flash, then install bootloader, table, and application; replaces previous firmware')
p.add_argument('--dry-run', action='store_true')
p.add_argument('--fast-backup', action='store_true', help='Use verified stub chunks; known to stall on this S2, ROM reads are the default')
p.add_argument('--skip-backup', action='store_true', help='Explicitly replace firmware without a full backup; requires --provision')
a = p.parse_args()
if a.skip_backup and not a.provision:
    p.error('--skip-backup requires --provision')
images = Path('firmware').resolve()
app = images / 'async-worker-s2.bin'
table = images / 'partition-table.bin'
boot = images / 'bootloader.bin'
for path in (app, table, boot):
    if not path.is_file():
        p.error(f'Missing {path}; run make firmware')
if not 0 < app.stat().st_size <= 0x180000:
    p.error('Application exceeds factory partition')
plan = ('SKIP BACKUP, replace bootloader/table/app' if a.skip_backup else 'backup 4 MiB, provision bootloader/table/app') if a.provision else 'verify table, update app at 0x10000'
print(f'ESP32-S2 on {a.port}: {plan}', flush=True)
if a.dry_run:
    raise SystemExit(0)
# Fixed --chip makes esptool reject an S3. Keep ROM loader running between
# commands because S2 native USB changes COM ports when the app boots.
tool = [sys.executable, '-m', 'esptool', '--chip', 'esp32s2', '--port', a.port]
with tempfile.TemporaryDirectory(prefix='async-worker-') as temp:
    if a.provision and not a.skip_backup:
        backup = images / ('backup-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '.bin')
        partial = backup.with_suffix('.partial')
        if not a.fast_backup:
            subprocess.run([*tool, '--no-stub', '--after', 'no_reset', 'read_flash',
                            '0', '0x400000', str(partial)], check=True, timeout=7200)
        else:
            # Bound each streaming operation. esptool verifies each chunk's
            # digest before returning. Never retry a stuck serial operation.
            with partial.open('xb') as output:
                for offset in range(0, 0x400000, 0x10000):
                    chunk = Path(temp) / 'chunk.bin'
                    subprocess.run([*tool, '--before', 'no_reset', '--after', 'no_reset_stub',
                                    'read_flash', hex(offset), '0x10000', str(chunk)],
                                   check=True, timeout=45)
                    data = chunk.read_bytes()
                    if len(data) != 0x10000:
                        raise SystemExit('Incomplete backup chunk; refusing to write')
                    output.write(data)
                    output.flush()
                    print(f'Backup verified: {offset + len(data)} / 4194304 bytes', flush=True)
        if partial.stat().st_size != 0x400000:
            raise SystemExit('Incomplete backup; refusing to write')
        partial.rename(backup)
        backup.with_suffix('.sha256').write_text(hashlib.sha256(backup.read_bytes()).hexdigest() + '  ' + backup.name + '\n')
        print(f'Full backup: {backup}')
    if a.provision:
        writes = ['0x1000', str(boot), '0x8000', str(table), '0x10000', str(app)]
    else:
        current = Path(temp) / 'partitions.bin'
        subprocess.run([*tool, '--after', 'no_reset_stub', 'read_flash', '0x8000', '0x1000', str(current)], check=True, timeout=45)
        if current.read_bytes()[:len(table.read_bytes())] != table.read_bytes():
            raise SystemExit('Partition layout differs. First installation requires --provision; no writes performed.')
        writes = ['0x10000', str(app)]
    subprocess.run([*tool, '--before', 'no_reset', 'write_flash', '--flash_size', '4MB', *writes], check=True, timeout=180)
print('Done. Native USB may reappear on another COM port. Tap RESET if still in ROM boot mode.')
