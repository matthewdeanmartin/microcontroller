"""The household's housemetrics board, for tools/boardsafe (image checks,
application-only updates and live probes). Replacing the board is a
deliberate edit to `mac`.
"""
from pathlib import Path

from boardsafe.boards import Board

ROOT = Path(__file__).resolve().parent

BOARDS = {
    's2': Board(
        id='s2', name='ESP32-S2 Mini (housemetrics)', app='housemetrics', chip='esp32s2',
        target='xtensa-esp32s2-espidf', flash_size='4MB', bootloader_offset=0x1000,
        hostname='housemetrics.local', mac='80:65:99:f0:7b:68', target_dir='C:/mfw-s2',
        binary='housemetrics-esp32', partitions='partitions.csv', web_dir='.embuild/web',
        root=ROOT, before='no_reset', cert='housemetrics', ca='certs/household-ca.crt'),
}
