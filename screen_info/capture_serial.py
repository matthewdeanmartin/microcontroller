"""Capture a bounded USB console session during board bring-up."""
import argparse
import time
from pathlib import Path
import serial

parser = argparse.ArgumentParser()
parser.add_argument('--port', default='COM17')
parser.add_argument('--seconds', type=float, default=15)
args = parser.parse_args()
console = serial.Serial()
console.port = args.port
console.baudrate = 115200
console.timeout = 0.2
console.dtr = False
console.rts = False
console.open()
chunks = []
try:
    deadline = time.monotonic() + args.seconds
    while time.monotonic() < deadline:
        data = console.read(console.in_waiting or 1)
        if data:
            chunks.append(data)
finally:
    console.close()
output = b''.join(chunks).decode('utf-8', errors='replace')
Path(__file__).with_name('serial.log').write_text(output, encoding='utf-8')
print(output)
