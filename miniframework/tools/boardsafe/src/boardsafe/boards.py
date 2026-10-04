"""The registry of an app's physical boards.

Replacing a physical board is a deliberate edit to its ``mac``. There is no
default board anywhere: every command names one.
"""
from __future__ import annotations

import csv
from dataclasses import dataclass
import importlib.util
from pathlib import Path

#: Partition table type and subtype numbers (ESP-IDF ``esp_partition.h``).
KINDS = {'app': 0, 'data': 1}
SUBTYPES = {
    'app': {'factory': 0x00, 'test': 0x20, **{f'ota_{n}': 0x10 + n for n in range(16)}},
    'data': {
        'ota': 0x00, 'phy': 0x01, 'nvs': 0x02, 'coredump': 0x03, 'nvs_keys': 0x04,
        'efuse': 0x05, 'undefined': 0x06, 'esphttpd': 0x80, 'fat': 0x81,
        'spiffs': 0x82, 'littlefs': 0x83,
    },
}

#: Espressif application-image header chip IDs, by esptool ``--chip`` name.
CHIP_IDS = {'esp32': 0, 'esp32s2': 2, 'esp32c3': 5, 'esp32s3': 9, 'esp32c6': 13}


@dataclass(frozen=True)
class Board:
    """One physical board running one app."""

    id: str
    name: str
    #: App name; the image marker is ``<APP>-BOARD:<id>:<hostname>;``.
    app: str
    #: esptool ``--chip``.
    chip: str
    #: Rust target triple, e.g. ``xtensa-esp32s3-espidf``.
    target: str
    flash_size: str
    bootloader_offset: int
    hostname: str
    #: This household's physical board.
    mac: str
    #: Cargo target directory (short Windows paths: esp-idf-sys needs them).
    target_dir: str
    #: Firmware binary name (Cargo ``[[bin]]``).
    binary: str
    #: Partition table CSV, relative to ``root``.
    partitions: str
    #: Bundled web directory (holds ``assets.rs``), relative to ``root``.
    web_dir: str
    #: The app's directory; relative paths above resolve against it.
    root: Path
    #: esptool ``--before`` for a session's first command. Native-USB boards
    #: (the S2) have no reset bridge: an operator puts them in download mode.
    before: str = 'default_reset'
    sdkconfig: str = 'sdkconfig.defaults'
    #: Server certificate base name (``certs/<cert>.crt``/``.key``).
    cert: str = ''
    #: The household CA (PEM) that signed it, relative to ``root``.
    ca: str = ''

    @property
    def manual_download(self) -> bool:
        return self.before == 'no_reset'

    @property
    def image_chip_id(self) -> int:
        return CHIP_IDS[self.chip]

    @property
    def marker(self) -> bytes:
        return f'{self.app.upper()}-BOARD:{self.id}:{self.hostname};'.encode('ascii')

    def layout(self) -> dict[str, tuple[int, int, int, int]]:
        """Partition rows as ``name: (type, subtype, offset, size)``."""
        return read_layout(self.root / self.partitions)

    def app_size(self) -> int:
        """The smallest application partition: what an image must fit."""
        sizes = [size for kind, _, _, size in self.layout().values() if kind == KINDS['app']]
        if not sizes:
            raise ValueError(f'{self.partitions} has no application partition')
        return min(sizes)

    def app_offset(self) -> int:
        """Where the update path writes: the factory (or first) app slot."""
        apps = sorted(offset for kind, _, offset, _ in self.layout().values() if kind == KINDS['app'])
        return apps[0]

    def image(self, target_dir: str | None = None) -> Path:
        return Path(target_dir or self.target_dir) / self.target / 'release' / f'{self.binary}.bin'


def number(text: str) -> int:
    """``0x9000``, ``4096``, ``4K`` or ``2M``."""
    text = text.strip()
    for suffix, scale in (('K', 1024), ('M', 1024 * 1024)):
        if text.upper().endswith(suffix):
            return int(text[:-1], 0) * scale
    return int(text, 0)


def read_layout(path: Path) -> dict[str, tuple[int, int, int, int]]:
    """Reads an ESP-IDF partition CSV. Offsets must be explicit: a deploy
    check compares exact rows, and an implied offset depends on the tool."""
    rows: dict[str, tuple[int, int, int, int]] = {}
    with Path(path).open(newline='') as stream:
        lines = (line for line in stream if line.strip() and not line.lstrip().startswith('#'))
        for row in csv.reader(lines):
            name, kind, subtype, offset, size = (field.strip() for field in row[:5])
            if not offset:
                raise ValueError(f'{path}: partition {name} needs an explicit offset')
            sub = SUBTYPES[kind].get(subtype)
            rows[name] = (KINDS[kind], sub if sub is not None else number(subtype), number(offset), number(size))
    return rows


def load(registry: str | Path) -> dict[str, Board]:
    """``BOARDS`` from an app's registry file."""
    path = Path(registry).resolve()
    spec = importlib.util.spec_from_file_location(f'boardsafe_registry_{abs(hash(path))}', path)
    if spec is None or spec.loader is None:
        raise SystemExit(f'Cannot load board registry {path}')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    boards = getattr(module, 'BOARDS', None)
    if not isinstance(boards, dict) or not boards:
        raise SystemExit(f'{path} defines no BOARDS')
    return boards


def pick(boards: dict[str, Board], name: str) -> Board:
    """A board by id; there is deliberately no default."""
    try:
        return boards[name]
    except KeyError:
        raise SystemExit(f'Unknown board {name!r}; choose one of: {", ".join(boards)}') from None
