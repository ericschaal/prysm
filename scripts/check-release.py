#!/usr/bin/env python3
"""Check the product version: one file carries it, and a tag must match it.

The product version is the `VERSION` file alone. Release Please owns it
(release-type `simple`) and also updates the one pinned copy, the image tag in
packaging/docker/compose.truenas.yaml (`x-release-please-version`). The Rust
code reads VERSION at compile time (crates/version); the crates themselves are
`0.0.0`, and the OpenAPI documents carry their API's version, not this one.
`.release-please-manifest.json` holds the last released version, so it is
compared only for a tag: by then the release pull request has set it.

Usage: check-release.py [TAG]   prints the version, or exits non-zero.
"""
import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
COMPOSE = 'packaging/docker/compose.truenas.yaml'
SEMVER = re.compile(
    r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)'
    r'(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?'
    r'(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?')
CRATE_VERSION = '0.0.0'


def release_version(root, tag=None):
    """VERSION, after checking that nothing else carries it; with a tag, also
    that the tag and the manifest match."""
    version = (root / 'VERSION').read_text().strip()
    if not SEMVER.fullmatch(version):
        raise ValueError(f'VERSION is not a semantic version: {version!r}')
    crates = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
    if crates != CRATE_VERSION:
        raise ValueError(f'[workspace.package] version is {crates}: keep it {CRATE_VERSION}; '
                         'the product version is VERSION alone')
    for directory in ('crates', 'adapters', 'surfaces', 'hosts', 'tools'):
        for source in sorted((root / directory).rglob('*.rs')):
            if 'target' in source.relative_to(root).parts:
                continue
            if 'CARGO_PKG_VERSION' in source.read_text():
                raise ValueError(f'{source.relative_to(root)} uses CARGO_PKG_VERSION (0.0.0); '
                                 'use prysm_version::VERSION')
    pins = re.findall(r'prysm-headless:(\S+) # x-release-please-version', (root / COMPOSE).read_text())
    if pins != [version]:
        raise ValueError(f'{COMPOSE} must pin prysm-headless:{version} # x-release-please-version, has {pins}')
    if tag is not None:
        if tag != f'v{version}':
            raise ValueError(f'tag {tag!r} must be v{version}')
        manifest = json.loads((root / '.release-please-manifest.json').read_text())['.']
        if manifest != version:
            raise ValueError(f'.release-please-manifest.json has {manifest}, the tag {tag}')
    return version


if __name__ == '__main__':
    try:
        print(release_version(ROOT, sys.argv[1] if len(sys.argv) > 1 else None))
    except (ValueError, KeyError, OSError) as error:
        sys.exit(f'check-release: {error}')
