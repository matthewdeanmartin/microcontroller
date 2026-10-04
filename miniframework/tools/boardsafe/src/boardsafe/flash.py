"""Update only the application on a board that is already installed.

Every write is preceded by four independent identity checks, any of which
stops the update before the flash is touched:

1. the image header's chip ID and embedded board marker name this board;
2. esptool connects with this board's --chip (another chip is refused);
3. the connected chip's MAC is the one recorded for this board;
4. the partition table on the chip is exactly this board's layout.

No full-chip erase, partition rewrite, bootloader replacement or serial
monitor: data partitions (a ledger, settings, certificates) are never
written. Usage:

  boardsafe-update --registry boards.py --board ID --port COM9 [--image PATH] [--dry-run]
"""
from __future__ import annotations

import argparse
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import time
from typing import Iterable

from .boards import Board, load, pick
from .image import check_image

PARTITION_TABLE = 0x8000


def parse_partition_table(data: bytes) -> dict[str, tuple[int, int, int, int]]:
    """Rows of a binary partition table as ``name: (type, subtype, offset, size)``."""
    found: dict[str, tuple[int, int, int, int]] = {}
    for at in range(0, min(len(data), 0xC00), 32):
        row = data[at:at + 32]
        if len(row) != 32:
            raise ValueError('Short partition table')
        (magic,) = struct.unpack_from('<H', row)
        if magic in (0xFFFF, 0xEBEB):  # end / IDF MD5 trailer
            break
        if magic != 0x50AA:
            raise ValueError('Invalid partition table')
        _, kind, subtype, offset, size, label, flags = struct.unpack('<HBBII16sI', row)
        name = label.rstrip(b'\0').decode('ascii', 'replace')
        if flags or name in found:
            raise ValueError('Encrypted or duplicate partition: needs a separate deployment review')
        found[name] = (kind, subtype, offset, size)
    if not found:
        raise ValueError('Empty partition table')
    return found


def verify_partition_table(board: Board, data: bytes, others: Iterable[Board] = ()) -> None:
    """The chip's table must be exactly this board's layout."""
    found = parse_partition_table(data)
    if found == board.layout():
        return
    for other in others:
        if other.id != board.id and found == other.layout():
            raise ValueError(
                f'This chip has the {other.id} layout ({other.hostname}), not {board.id}; refusing to write.'
            )
    raise ValueError(
        f'Board is not using the {board.id} partition layout; refusing to write. No automatic migration.'
    )


def chip_mac(output: str) -> str:
    match = re.search(r'MAC:\s*([0-9a-fA-F]{2}(?::[0-9a-fA-F]{2}){5})', output)
    if not match:
        raise ValueError('esptool did not report the chip MAC; refusing to write.')
    return match.group(1).lower()


def verify_mac(board: Board, output: str, others: Iterable[Board] = ()) -> str:
    """The MAC esptool reported must be this board's."""
    mac = chip_mac(output)
    if mac == board.mac.lower():
        return mac
    for other in others:
        if other.mac.lower() == mac:
            raise ValueError(
                f'Port is the {other.id} board ({other.hostname}, MAC {mac}), not {board.id}; refusing to write.'
            )
    raise ValueError(
        f'Chip MAC {mac} is not the recorded {board.id} board ({board.mac}); refusing to write. '
        'If the board was deliberately replaced, update the registry first.'
    )


def wait_for_port(port: str, seconds: float = 20) -> None:
    """A native-USB board re-enumerates when esptool closes it, even while it
    stays in download mode. Wait for the same port to return."""
    from serial.tools import list_ports

    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if any(p.device.upper() == port.upper() for p in list_ports.comports()):
            time.sleep(0.5)  # let Windows finish attaching the driver
            return
        time.sleep(0.25)
    raise SystemExit(
        f'{port} did not return within {seconds:.0f} s; nothing further was written. '
        'Put the board back in download mode and check the port.'
    )


def esptool(board: Board, port: str, *args: str, capture: bool = False) -> str:
    if board.manual_download:
        wait_for_port(port)
    command = [sys.executable, '-m', 'esptool', '--chip', board.chip, '--port', port, *args]
    if not capture:
        subprocess.run(command, check=True)
        return ''
    result = subprocess.run(command, check=False, capture_output=True, text=True)
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    if result.returncode:
        raise SystemExit(f'esptool failed (exit {result.returncode}); nothing was written.')
    return result.stdout


def read_identity(board: Board, port: str, folder: str, others: Iterable[Board]) -> tuple[str, bytes]:
    """One esptool session: chip type (by --chip), MAC and partition table."""
    table = Path(folder) / 'partitions.bin'
    output = esptool(
        board, port, '--before', board.before, '--after', 'no_reset',
        'read_flash', hex(PARTITION_TABLE), '0x1000', str(table), capture=True,
    )
    return verify_mac(board, output, others), table.read_bytes()


def update(board: Board, port: str, image: Path, others: Iterable[Board], dry_run: bool = False) -> None:
    others = list(others)
    try:
        size = check_image(board, image, others)
    except (OSError, ValueError) as error:
        raise SystemExit(str(error)) from None
    offset = board.app_offset()
    print(f'Board: {board.id} = {board.name}, {board.hostname}, chip {board.chip}, MAC {board.mac}')
    print(f'Plan: verify chip, MAC and {board.id} partition table on {port}, '
          f'then write {image} ({size} bytes) at {offset:#x} only.')
    print('Data partitions, bootloader and partition table will not be written.')
    if board.manual_download:
        print('The board must already be in download mode: hold BOOT, tap RST, release BOOT.')
    if dry_run:
        return
    with tempfile.TemporaryDirectory(prefix='boardsafe-') as folder:
        try:
            mac, table = read_identity(board, port, folder, others)
            verify_partition_table(board, table, others)
        except ValueError as error:
            raise SystemExit(str(error)) from None
    print(f'Verified {board.id}: MAC {mac}, {board.id} partition layout.')
    esptool(board, port, '--before', 'no_reset', '--after', 'hard_reset', 'write_flash', hex(offset), str(image))
    print(f'Application updated on {board.id}. Open https://{board.hostname}/ after restart.')


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--registry', required=True, type=Path)
    parser.add_argument('--board', required=True)
    parser.add_argument('--port', required=True)
    parser.add_argument('--image', type=Path, help='defaults to the board target directory')
    parser.add_argument('--dry-run', action='store_true', help='check the image and print the plan; no serial port')
    args = parser.parse_args(argv)
    boards = load(args.registry)
    board = pick(boards, args.board)
    update(board, args.port, (args.image or board.image()).resolve(), boards.values(), args.dry_run)


if __name__ == '__main__':
    main()
