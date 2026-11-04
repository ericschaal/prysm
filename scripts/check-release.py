#!/usr/bin/env python3
"""Verify that a release tag and its installation packages share one version."""
import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def release_version(root, tag=None):
    version = (root / 'VERSION').read_text().strip()
    if not re.fullmatch(
            r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version):
        raise ValueError('VERSION must contain a stable semantic version')
    if tag is not None and tag != f'v{version}':
        raise ValueError(f'tag {tag!r} must match v{version}')
    cargo = tomllib.loads((root / 'apps/prysm/Cargo.toml').read_text())
    lock = tomllib.loads((root / 'Cargo.lock').read_text())
    app = json.loads((root / 'crates/web/prysm-webos/public/appinfo.json').read_text())
    manifest = json.loads((root / '.release-please-manifest.json').read_text())
    versions = [cargo['package']['version'], app['version'], manifest['.']]
    versions.extend(p['version'] for p in lock['package'] if p['name'] == 'prysm')
    if len(versions) != 4 or any(value != version for value in versions):
        raise ValueError(f'release version mismatch: VERSION={version}, manifests={versions}')
    for name in ('compose.yaml', 'packaging/docker/compose.truenas.yaml'):
        if f'image: ghcr.io/ericschaal/prysm:{version} ' not in (root / name).read_text():
            raise ValueError(f'{name} must pin the release image')
    return version


if __name__ == '__main__':
    try:
        print(release_version(ROOT, sys.argv[1] if len(sys.argv) > 1 else None))
    except ValueError as error:
        sys.exit(str(error))
