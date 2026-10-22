"""Offline, fail-closed partition gate for tools/flash.sh."""
import argparse
import hashlib
from pathlib import Path

from .flash import parse_partition_table


def checked_table(data: bytes) -> dict:
    if len(data) < 0xC00 or len(data) % 32:
        raise ValueError('Short or misaligned partition table')
    for at in range(0, 0xC00, 32):
        row = data[at:at + 32]
        if row[:2] == b'\xeb\xeb':
            if row[:16] != b'\xeb\xeb' + b'\xff' * 14 or row[16:] != hashlib.md5(data[:at]).digest():
                raise ValueError('Partition table checksum mismatch')
            break
        if row == b'\xff' * 32:
            break
    else:
        raise ValueError('Unterminated partition table')
    return parse_partition_table(data)


def verify(existing: bytes, expected: bytes, replace_nanacoin: bool = False) -> None:
    found = checked_table(existing)
    wanted = checked_table(expected)
    if 'ledger' in found:
        if not replace_nanacoin:
            raise ValueError('NanaCoin ledger present; explicit --replace-nanacoin required.')
        # A migration needs its own reviewed layout, not just a label match.
        if found != wanted:
            raise ValueError('Ledger layout differs; use a separately reviewed migration.')
    if found != wanted:
        raise ValueError('Unknown partition layout; refusing to overwrite it.')


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('existing', type=Path)
    parser.add_argument('expected', type=Path)
    parser.add_argument('--replace-nanacoin', action='store_true')
    args = parser.parse_args()
    try:
        verify(args.existing.read_bytes(), args.expected.read_bytes(), args.replace_nanacoin)
    except (ValueError, OSError) as error:
        raise SystemExit(f'{error} Nothing was written.') from None


if __name__ == '__main__':
    main()
