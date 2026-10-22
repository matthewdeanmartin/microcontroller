"""First-install regressions: never touch hardware."""
import hashlib
import struct

import pytest

from boardsafe.first_install import verify


def table(name='factory', offset=0x10000):
    row = struct.pack('<HBBII16sI', 0x50AA, 0, 0, offset, 0x200000, name.encode(), 0)
    checksum = b'\xeb\xeb' + b'\xff' * 14 + hashlib.md5(row).digest()
    return (row + checksum).ljust(0xC00, b'\xff')


def test_matching_layout():
    verify(table(), table())


@pytest.mark.parametrize('bad', [b'', b'\xff' * 0xC00, b'\0' * 0xC00,
                                     table()[:32], table(offset=0x20000), table('unknown')])
@pytest.mark.parametrize('override', [False, True])
def test_unknown_empty_and_truncated_tables_are_refused(bad, override):
    with pytest.raises(ValueError):
        verify(bad, table(), override)


def test_corrupt_checksum():
    bad = bytearray(table())
    bad[48] ^= 1
    with pytest.raises(ValueError, match='checksum'):
        verify(bytes(bad), table())


def test_ledger_override_does_not_authorize_unknown_migration():
    with pytest.raises(ValueError, match='explicit'):
        verify(table('ledger'), table())
    with pytest.raises(ValueError, match='migration'):
        verify(table('ledger'), table(), True)


def test_same_ledger_layout_still_requires_override():
    with pytest.raises(ValueError, match='explicit'):
        verify(table('ledger'), table('ledger'))
    verify(table('ledger'), table('ledger'), True)
