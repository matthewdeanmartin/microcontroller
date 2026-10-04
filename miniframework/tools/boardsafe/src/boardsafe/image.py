"""Is this application image the one built for this board?

Usage: boardsafe-image --registry boards.py --board ID [--image PATH]
       (converts nothing; checks a ``.bin`` made by esptool elf2image)
"""
from __future__ import annotations

import argparse
from pathlib import Path
from typing import Iterable

from .boards import Board, load, pick


def check_image(board: Board, image: Path, others: Iterable[Board] = ()) -> int:
    """Refuses an image that is not this board's build. Returns its size.

    The header's chip ID must be this board's chip; the image must embed this
    board's marker and no other board's; it must fit the app partition.
    """
    data = Path(image).read_bytes()
    if len(data) < 24 or data[0] != 0xE9:
        raise ValueError(f'{image} is not an Espressif application image')
    chip_id = int.from_bytes(data[12:14], 'little')
    if chip_id != board.image_chip_id:
        raise ValueError(
            f'{image} is for chip ID {chip_id}, not {board.chip} ({board.image_chip_id}); refusing.'
        )
    if board.marker not in data:
        raise ValueError(f'{image} lacks the {board.id} board marker ({board.hostname}); refusing.')
    for other in others:
        if other.id != board.id and other.marker in data:
            raise ValueError(f'{image} is marked for board {other.id} ({other.hostname}); refusing.')
    limit = board.app_size()
    if len(data) > limit:
        raise ValueError(
            f'Firmware is {len(data)} bytes; the {board.id} app partition is {limit} bytes. Refusing deployment.'
        )
    return len(data)


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--registry', required=True, type=Path)
    parser.add_argument('--board', required=True)
    parser.add_argument('--image', type=Path, help='defaults to the board target directory')
    args = parser.parse_args(argv)
    boards = load(args.registry)
    board = pick(boards, args.board)
    image = args.image or board.image()
    try:
        size = check_image(board, image, boards.values())
    except (OSError, ValueError) as error:
        raise SystemExit(str(error)) from None
    print(f'Board {board.id} firmware: {image} ({size} / {board.app_size()} bytes, {board.hostname}). No board accessed.')


if __name__ == '__main__':
    main()
