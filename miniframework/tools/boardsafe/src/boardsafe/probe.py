"""Strict live check of one board after an update.

Verifies TLS against the household CA with the board's own hostname (no
bypass), that the board answering is the expected app and host (so another
board at that address fails), that it serves this build's site byte for
byte, and that ``/ca`` is the CA that verified it. Usage:

  boardsafe-probe --registry boards.py --board ID --address 192.168.1.158 [--ca certs/ca.crt]

Apps add their own checks on top with :func:`probe` and :func:`get`.
"""
from __future__ import annotations

import argparse
import gzip
import http.client
import json
from pathlib import Path
import socket
import ssl
import time

from .assets import bundled_assets, verify_board_assets
from .boards import Board, load, pick


def context_for(ca_pem: Path) -> ssl.SSLContext:
    context = ssl.create_default_context(cafile=str(ca_pem))
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    return context


def get(address: str, hostname: str, context: ssl.SSLContext, path: str):
    """One strict-TLS GET (gzip accepted, decoded). Returns (status, body, cipher)."""
    raw = socket.create_connection((address, 443), timeout=5)
    tls = context.wrap_socket(raw, server_hostname=hostname)
    cipher = tls.cipher()
    tls.sendall(
        f'GET {path} HTTP/1.1\r\nHost: {hostname}\r\nAccept-Encoding: gzip\r\n'
        'Connection: close\r\n\r\n'.encode()
    )
    response = http.client.HTTPResponse(tls)
    response.begin()
    body = response.read()
    if response.getheader('Content-Encoding') == 'gzip':
        body = gzip.decompress(body)
    status = response.status
    tls.close()
    return status, body, cipher


def bundled_index(web_dir: Path) -> bytes:
    for uri, identity, _, _ in bundled_assets(web_dir):
        if uri == '/index.html':
            return identity
    raise SystemExit('Could not identify the locally bundled index.html')


def check_identity(board: Board, info: dict) -> None:
    """The board's ``/api/v1/sys`` must name this app and host."""
    if info.get('app') != board.app or info.get('host') != board.hostname:
        raise SystemExit(
            f"Board answered as {info.get('app')!r} on {info.get('host')!r}, "
            f'not {board.app} on {board.hostname}. Stop: wrong board at this address.'
        )


def probe(board: Board, address: str, ca_pem: Path, attempts: int = 15) -> dict:
    """Runs every generic check; returns the board's ``/api/v1/sys``."""
    context = context_for(ca_pem)
    last_error = None
    for _ in range(attempts):
        try:
            status, body, cipher = get(address, board.hostname, context, '/api/v1/sys?fmt=json')
            break
        except (OSError, ssl.SSLError) as error:
            last_error = error
            time.sleep(2)
    else:
        raise SystemExit(f'Board did not pass strict TLS after {attempts} attempts: {last_error}')
    if status != 200:
        raise SystemExit(f'/api/v1/sys returned HTTP {status}')
    info = json.loads(body)
    check_identity(board, info)

    web_dir = board.root / board.web_dir
    status, site, _ = get(address, board.hostname, context, '/')
    if status != 200 or site != bundled_index(web_dir):
        raise SystemExit('Board is serving a site different from this build')

    status, served_ca, _ = get(address, board.hostname, context, '/ca')
    expected = ssl.PEM_cert_to_DER_cert(Path(ca_pem).read_text())
    if status != 200 or served_ca != expected:
        raise SystemExit('Board /ca does not match the CA used to verify its certificate')

    count = verify_board_assets(address, board.hostname, context, web_dir)
    gzip_only = bundled_assets(web_dir)[0][3]
    info['_probe'] = {
        'cipher': cipher[0],
        'assets': count,
        'encodings': 'gzip-only + 406 identity' if gzip_only else 'identity + gzip',
    }
    return info


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--registry', required=True, type=Path)
    parser.add_argument('--board', required=True)
    parser.add_argument('--address', required=True, help='board IP address or resolvable name')
    parser.add_argument('--ca', type=Path, help='household CA (PEM); defaults to the board registry')
    parser.add_argument('--attempts', type=int, default=15)
    args = parser.parse_args(argv)
    board = pick(load(args.registry), args.board)
    ca = args.ca or (board.root / board.ca)
    info = probe(board, args.address, ca, args.attempts)
    detail = info['_probe']
    print(
        f"Probe passed for {board.id} ({board.hostname}, {info.get('platform')}, build {info.get('build')}): "
        f"strict CA/hostname TLS ({detail['cipher']}), identity, {detail['assets']} exact assets "
        f"({detail['encodings']}, ETags, keep-alive and slow-reader checks) and matching /ca."
    )


if __name__ == '__main__':
    main()
