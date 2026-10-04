"""Offline safety tests: no test runs esptool or opens a serial port."""
from pathlib import Path
import struct

import pytest

from boardsafe.boards import Board, load, number, pick, read_layout
from boardsafe.flash import parse_partition_table, verify_mac, verify_partition_table
from boardsafe.image import check_image

S3_CSV = """# name, type, subtype, offset, size
nvs, data, nvs, 0x9000, 0x6000,
phy_init, data, phy, 0xf000, 0x1000,
factory, app, factory, 0x10000, 0x400000,
ledger, data, nvs, 0x410000, 0x800000,
"""
S2_CSV = """nvs, data, nvs, 0x9000, 0x6000,
phy_init, data, phy, 0xf000, 0x1000,
factory, app, factory, 0x10000, 0x260000,
ledger, data, nvs, 0x270000, 0x190000,
"""
OTA_CSV = """nvs, data, nvs, 0x9000, 0x6000,
phy_init, data, phy, 0xf000, 0x1000,
ota_0, app, ota_0, 0x10000, 0x1E0000,
ota_1, app, ota_1, 0x1F0000, 0x1D0000,
otadata, data, ota, 0x3D0000, 0x2000,
coredump, data, coredump, 0x3F0000, 64K,
"""


@pytest.fixture
def boards(tmp_path: Path):
    (tmp_path / 's3.csv').write_text(S3_CSV)
    (tmp_path / 's2.csv').write_text(S2_CSV)
    common = dict(app='bank', flash_size='4MB', binary='bank-esp32', web_dir='web', root=tmp_path)
    s3 = Board(id='s3', name='S3', chip='esp32s3', target='xtensa-esp32s3-espidf', bootloader_offset=0,
               hostname='bank.local', mac='ac:a7:04:2c:2c:04', target_dir='C:/b3', partitions='s3.csv', **common)
    s2 = Board(id='s2', name='S2', chip='esp32s2', target='xtensa-esp32s2-espidf', bootloader_offset=0x1000,
               hostname='bank-s2.local', mac='80:65:99:f0:1c:9c', target_dir='C:/b2', partitions='s2.csv',
               before='no_reset', **common)
    return {'s3': s3, 's2': s2}


def table(rows: dict) -> bytes:
    return b''.join(
        struct.pack('<HBBII16sI', 0x50AA, *values, name.encode(), 0) for name, values in rows.items()
    ) + b'\xff' * 32


def image(board: Board, marker: bytes | None = None, size: int = 4096) -> bytes:
    header = bytearray(b'\xe9' + b'\0' * 23)
    header[12:14] = board.image_chip_id.to_bytes(2, 'little')
    body = bytes(header) + (board.marker if marker is None else marker)
    return body + b'\0' * (size - len(body))


def test_layout_and_sizes(boards, tmp_path):
    s3, s2 = boards['s3'], boards['s2']
    assert s3.layout()['ledger'] == (1, 2, 0x410000, 0x800000)
    assert s2.app_size() == 0x260000 and s2.app_offset() == 0x10000
    assert max(o + s for _, _, o, s in s2.layout().values()) == 0x400000
    (tmp_path / 'ota.csv').write_text(OTA_CSV)
    ota = read_layout(tmp_path / 'ota.csv')
    assert ota['ota_1'][:2] == (0, 0x11) and ota['otadata'][:2] == (1, 0)
    assert ota['coredump'][3] == 0x10000
    # An image must fit the smaller slot.
    from dataclasses import replace
    assert replace(s2, partitions='ota.csv').app_size() == 0x1D0000
    assert number('2M') == 0x200000 and number('0x10') == 16


def test_implicit_offsets_are_refused(tmp_path):
    (tmp_path / 'auto.csv').write_text('nvs, data, nvs, , 0x6000,\n')
    with pytest.raises(ValueError, match='explicit offset'):
        read_layout(tmp_path / 'auto.csv')


def test_markers_and_registry_are_distinct(boards):
    s3, s2 = boards['s3'], boards['s2']
    assert s3.marker == b'BANK-BOARD:s3:bank.local;'
    assert s3.marker not in s2.marker and s2.marker not in s3.marker
    with pytest.raises(SystemExit, match='choose one of'):
        pick(boards, 's4')


def test_partition_tables(boards):
    s3, s2 = boards['s3'], boards['s2']
    for board in boards.values():
        verify_partition_table(board, table(board.layout()), boards.values())
    with pytest.raises(ValueError, match='has the s2 layout'):
        verify_partition_table(s3, table(s2.layout()), boards.values())
    for board in boards.values():
        for name in board.layout():
            modified = dict(board.layout())
            del modified[name]
            with pytest.raises(ValueError):
                verify_partition_table(board, table(modified), boards.values())
    moved = dict(s3.layout())
    moved['ledger'] = (1, 2, 0x210000, 0x800000)
    with pytest.raises(ValueError, match='No automatic migration'):
        verify_partition_table(s3, table(moved))
    for bad in (b'', b'x' * 32, b'\xff' * 4096):
        with pytest.raises(ValueError):
            verify_partition_table(s3, bad)
    encrypted = table(s3.layout())[:28] + struct.pack('<I', 1) + table(s3.layout())[32:]
    with pytest.raises(ValueError, match='Encrypted'):
        parse_partition_table(encrypted)


def test_mac_must_be_the_recorded_board(boards):
    s3, s2 = boards['s3'], boards['s2']
    assert verify_mac(s2, f'Chip is ESP32-S2\nMAC: {s2.mac.upper()}\n', boards.values()) == s2.mac
    with pytest.raises(ValueError, match='is the s3 board'):
        verify_mac(s2, f'MAC: {s3.mac}\n', boards.values())
    with pytest.raises(ValueError, match='not the recorded'):
        verify_mac(s3, 'MAC: 00:11:22:33:44:55\n', boards.values())
    with pytest.raises(ValueError, match='did not report'):
        verify_mac(s3, 'nothing', boards.values())


def test_image_must_be_this_boards_build(boards, tmp_path):
    s3, s2 = boards['s3'], boards['s2']
    path = tmp_path / 'app.bin'
    for board in boards.values():
        path.write_bytes(image(board))
        assert check_image(board, path, boards.values()) == 4096
    path.write_bytes(image(s2))
    with pytest.raises(ValueError, match='chip ID'):
        check_image(s3, path, boards.values())
    path.write_bytes(image(s3, marker=s2.marker))
    with pytest.raises(ValueError, match='lacks the s3 board marker'):
        check_image(s3, path, boards.values())
    path.write_bytes(image(s2, marker=s2.marker + s3.marker))
    with pytest.raises(ValueError, match='marked for board s3'):
        check_image(s2, path, boards.values())
    path.write_bytes(image(s2, size=s2.app_size() + 1))
    with pytest.raises(ValueError, match='app partition'):
        check_image(s2, path, boards.values())
    path.write_bytes(b'not an image' * 4)
    with pytest.raises(ValueError, match='not an Espressif'):
        check_image(s2, path)


def test_registry_files_load(tmp_path):
    (tmp_path / 'p.csv').write_text(S2_CSV)
    registry = tmp_path / 'boards.py'
    registry.write_text(
        'from pathlib import Path\n'
        'from boardsafe.boards import Board\n'
        "BOARDS = {'s2': Board(id='s2', name='S2', app='hm', chip='esp32s2', target='t', flash_size='4MB',"
        " bootloader_offset=0x1000, hostname='hm.local', mac='80:65:99:f0:7b:68', target_dir='C:/x',"
        " binary='hm', partitions='p.csv', web_dir='web', root=Path(__file__).parent)}\n"
    )
    boards = load(registry)
    assert boards['s2'].layout()['factory'][2] == 0x10000
    (tmp_path / 'empty.py').write_text('BOARDS = {}\n')
    with pytest.raises(SystemExit, match='defines no BOARDS'):
        load(tmp_path / 'empty.py')
