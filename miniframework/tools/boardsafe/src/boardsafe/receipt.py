"""Local baseline of a board's identity and build after a verified probe."""
from datetime import datetime, timezone
import json
from pathlib import Path


def write_receipt(path: Path, info: dict, *, image_sha256: str | None = None) -> str:
    host, build = info.get('host'), info.get('build')
    if not isinstance(host, str) or not isinstance(build, str) or not host or not build or build == 'dev':
        raise ValueError('A concrete host and build fingerprint are required for a baseline')
    if any(c.isspace() or c in '=\r\n' for c in host + build):
        raise ValueError('Invalid host/build in receipt')
    baseline = f'{host}={build}'
    receipt = {'schema': 'boardsafe.receipt/1', 'verified_at': datetime.now(timezone.utc).isoformat(),
               'app': info.get('app'), 'host': host, 'build': build, 'baseline': baseline,
               'deployed_image_sha256': image_sha256}
    tmp = path.with_suffix(path.suffix + '.tmp')
    tmp.write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
    tmp.replace(path)
    return baseline
