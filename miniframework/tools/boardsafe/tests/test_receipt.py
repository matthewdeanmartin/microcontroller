import json

import pytest

from boardsafe.receipt import write_receipt


def test_receipt_records_verified_build_and_replaces_baseline(tmp_path):
    path = tmp_path / 'board.json'
    info = {'app': 'board', 'host': 'board.local', 'build': 'abc123'}
    assert write_receipt(path, info, image_sha256='a' * 64) == 'board.local=abc123'
    assert json.loads(path.read_text())['deployed_image_sha256'] == 'a' * 64
    info['build'] = 'def456'
    write_receipt(path, info)
    assert json.loads(path.read_text())['baseline'] == 'board.local=def456'


@pytest.mark.parametrize('build', ['', 'dev', None, 'fake\nboard=x'])
def test_unknown_or_injected_build_cannot_replace_a_baseline(tmp_path, build):
    path = tmp_path / 'board.json'
    path.write_text('original')
    with pytest.raises(ValueError):
        write_receipt(path, {'host': 'board.local', 'build': build})
    assert path.read_text() == 'original'
