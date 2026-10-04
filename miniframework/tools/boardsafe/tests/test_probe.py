"""Probe logic without a board: identity, bundle parsing, asset verification."""
import gzip
from pathlib import Path

import pytest

from boardsafe.assets import bundled_assets, verify_asset
from boardsafe.boards import Board
from boardsafe.probe import check_identity


def board(tmp_path: Path) -> Board:
    return Board(id='s2', name='S2', app='housemetrics', chip='esp32s2', target='t', flash_size='4MB',
                 bootloader_offset=0x1000, hostname='housemetrics.local', mac='80:65:99:f0:7b:68',
                 target_dir='C:/x', binary='hm', partitions='p.csv', web_dir='web', root=tmp_path)


def test_identity_must_be_this_app_and_host(tmp_path):
    b = board(tmp_path)
    check_identity(b, {'app': 'housemetrics', 'host': 'housemetrics.local'})
    with pytest.raises(SystemExit, match='wrong board'):
        check_identity(b, {'app': 'nanacoin', 'host': 'nanacoin.local'})
    with pytest.raises(SystemExit, match='wrong board'):
        check_identity(b, {'app': 'housemetrics', 'host': 'housemetrics-2.local'})


def write_bundle(folder: Path, gzip_only: bool) -> bytes:
    folder.mkdir()
    html = b'<html><app-root></app-root></html>'
    (folder / '0.raw').write_bytes(b'' if gzip_only else html)
    (folder / '0.gz').write_bytes(gzip.compress(html))
    raw = (folder / '0.raw').as_posix()
    gz = (folder / '0.gz').as_posix()
    (folder / 'assets.rs').write_text(
        f'static ASSETS: &[Asset] = &[\n'
        f'Asset {{ path: "/index.html", mime: "text/html", raw: include_bytes!("{raw}"), '
        f'gzip: include_bytes!("{gz}"), etag: "W/\\"1\\"", immutable: false }}\n];\n'
    )
    return html


@pytest.mark.parametrize('gzip_only', [False, True])
def test_bundles_parse_with_identity_recovered(tmp_path, gzip_only):
    html = write_bundle(tmp_path / 'web', gzip_only)
    [(uri, identity, zipped, only)] = bundled_assets(tmp_path / 'web')
    assert (uri, identity, only) == ('/index.html', html, gzip_only)
    assert gzip.decompress(zipped) == html


def fake_board(asset, *, etag='W/"1"', drop_vary=False, truncate=False):
    """A getter that answers like a correct board, with optional faults."""
    uri, raw, zipped, gzip_only = asset

    def get(path, headers):
        assert path == uri
        encoding = headers.get('Accept-Encoding')
        if encoding == 'identity' and gzip_only:
            return 406, {}, b''
        body = zipped if encoding == 'gzip' else raw
        fields = {'etag': etag, 'content-length': str(len(body)),
                  'vary': '' if drop_vary else 'Accept-Encoding'}
        if encoding == 'gzip':
            fields['content-encoding'] = 'gzip'
        if headers.get('If-None-Match') == etag:
            return 304, {'etag': etag}, b''
        return 200, fields, body[:-1] if truncate else body

    return get


@pytest.mark.parametrize('gzip_only', [False, True])
def test_a_correct_board_passes_and_faults_are_caught(tmp_path, gzip_only):
    write_bundle(tmp_path / 'web', gzip_only)
    [asset] = bundled_assets(tmp_path / 'web')
    verify_asset(fake_board(asset), asset)
    with pytest.raises(RuntimeError, match='expected'):
        verify_asset(fake_board(asset, truncate=True), asset)
    with pytest.raises(RuntimeError, match='Vary'):
        verify_asset(fake_board(asset, drop_vary=True), asset)
    with pytest.raises(RuntimeError, match='ETag'):
        verify_asset(fake_board(asset, etag=''), asset)
