# Contribution guidelines

## Code

Keep applications under `apps/` and reusable libraries under
`crates/{capture,processing,rendering,common,web}/`. Use the existing boundaries
and types; see the [workspace map](architecture.md#workspace-map).
`prysm-core` must not depend on application, serialization or runtime code.
Avoid new dependencies for small helpers.

Capture adapters implement `PrysmCapturer` and preserve terminal errors separately
from replaceable frames. Do not block the HTTP/control runtime with capture or
native I/O. Keep best-effort frame traffic separate from ordered lifecycle commands.
Preserve validation, cleanup, timeouts, and [error sources](error-handling.md).

`prysm-contract` owns validated disk/HTTP models. After changing routes or DTOs,
[regenerate and test the frontend client](frontend.md#api-generation).
Browser settings are one complete typed JSON document. CLI/environment configure
only the process on every startup; neither group overrides the other. Keep
network bindings and allowed hosts outside application settings. Do not add a
database, generic settings resolver or separate setup-completion marker.

Update affected callers, fixtures, examples and documentation together. Test
behavior at the changed boundary. Preserve WLED stop/drain/join ordering and
failure compensation; see the [control API](control-api.md#lighting-transitions).

## Documentation

Each topic has one authoritative home:

- **[Users](../users/README.md):** installation, running capturers, configuration,
  and common fixes. Lead with prerequisites, numbered steps, and a success check.
  Use portable examples, show the explicit start action, and link out for details.
- **[Developers](README.md):** current contracts, architecture, technical decisions,
  debugging and contribution rules. Explain ownership, failure behavior and validation.
  Component READMEs hold narrow implementation contracts and link here for shared setup.
- **[Research](../../research/README.md):** experiments, alternatives and measurements.
  Label date, status, tested platform,
  findings and limits. Link to maintained guides for current behavior.

The root README introduces Prysm and points into these paths. Agent guidance links
here instead of repeating the architecture.
Keep public links working when relocating a guide.
The Debian package README is a self-contained installation handout; preserve its
runtime instructions when changing packaging.

Keep research evidence beside its scripts. Do not link to ignored `work/`, build
outputs, machine-specific absolute paths, or temporary files as public evidence.
Link to committed evidence that is available in a fresh checkout. Keep maintained
guides focused on the current code; omit migration and compatibility notes for
unreleased versions.

When moving a page, update incoming links and its relative paths. Check local
paths, images and heading anchors with:

```sh
python3 -m unittest discover -s scripts/tests
python3 scripts/check-doc-links.py
```

The [documentation workflow](../../.github/workflows/documentation.yml) checks
repository links and public URLs on documentation changes and can also run manually.
Runtime localhost/LAN endpoints are examples,
not public sites. Investigate failures rather than broadly excluding domains.
