"""What the report records about an artifact under test: its hash and age, and what
`cargo xtask dist` wrote about it in `manifest.json` next to it (docs/developers/packaging.md).

The manifest counts only when its hash matches the file. The runner reads no
version or commit of its own: without a matching manifest they are unknown.
"""

from dataclasses import dataclass
import datetime
import hashlib
import json
from pathlib import Path
from typing import TypedDict, cast

from prysm_acceptance import commands
from prysm_acceptance.transport import Transport


class Entry(TypedDict):
    """One artifact in `manifest.json` (tools/xtask/src/manifest.rs), the fields read here."""

    version: str
    commit: str
    dirty: bool
    sha256: str


@dataclass(frozen=True)
class Artifact:
    path: Path
    sha256: str
    modified: datetime.datetime
    entry: Entry | None  # from a manifest whose hash matches; None otherwise

    @property
    def version(self) -> str | None:
        return self.entry['version'] if self.entry else None

    def facts(self) -> dict[str, str]:
        age = datetime.datetime.now(datetime.UTC) - self.modified
        found = {
            'artifact': str(self.path),
            'sha256': self.sha256,
            'modified': f'{self.modified:%Y-%m-%d %H:%M} UTC ({age.total_seconds() / 3600:.1f} h ago)',
        }
        if self.entry:
            dirty = ' (built from a tree with uncommitted changes)' if self.entry['dirty'] else ''
            found |= {'version': self.entry['version'], 'commit': self.entry['commit'] + dirty}
        else:
            found['manifest'] = 'none matching: version and commit unknown'
        return found


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as file:
        for block in iter(lambda: file.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def inspect(path: Path) -> Artifact:
    """The artifact at `path` (which must exist), with its manifest entry when the hash matches."""
    found = sha256(path)
    modified = datetime.datetime.fromtimestamp(path.stat().st_mtime, datetime.UTC)
    manifest = path.with_name('manifest.json')
    entry = None
    if manifest.is_file():
        artifacts = cast('dict[str, dict[str, Entry]]', json.loads(manifest.read_text())).get('artifacts', {})
        candidate = artifacts.get(path.name)
        if candidate and candidate.get('sha256') == found:
            entry = candidate
    return Artifact(path, found, modified, entry)


def older_than_checkout(artifact: Artifact, transport: Transport) -> str | None:
    """A warning when the artifact is older than the checkout's last commit; None otherwise or when unknown."""
    result = transport.run(commands.git_last_commit_time(), check=False)
    try:
        committed = datetime.datetime.fromisoformat(result.stdout.strip())
    except ValueError:
        return None
    if artifact.modified >= committed:
        return None
    return f'{artifact.path.name} is older than the last commit ({committed:%Y-%m-%d %H:%M %z}): rebuild it?'
