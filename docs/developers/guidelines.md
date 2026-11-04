# Contribution guidelines

## Code

- Reuse existing boundaries and types; keep application/runtime/serialization
  dependencies out of `prysm-core`.
- Preserve typed validation and [error sources](error-handling.md).
- Keep capture/native I/O off the control runtime. Frames may be replaced;
  lifecycle commands must remain ordered.
- Preserve [stop/drain/join ordering](architecture.md#shutdown-ordering), WLED
  compensation and native watchdogs.
- Keep browser JSON settings separate from CLI/environment process options.
- Change callers, fixtures and docs together. Test the changed behavior.
- After DTO/route changes, [regenerate contracts](frontend.md#api-generation).

## Documentation

[Users](../users/README.md): steps to working lights.
[Developers](README.md): current architecture, commands and contracts.
[Research](../../research/README.md): experiments and measurements.

Write short, action-first pages: prerequisites, exact commands/clicks, expected result.
Cut promotion, vague steps, implementation narration in user guides, and repetition.

Give each topic one home; link instead of repeating it. Keep public paths/anchors
working. Maintained guides describe current code, without unreleased-version
migration notes. Link only committed evidence, never ignored build/work/temp files.
Keep the Debian package handout self-contained.

```sh
python3 -m unittest discover -s scripts/tests
python3 scripts/check-doc-links.py
```

[CI](../../.github/workflows/documentation.yml) also checks public URLs.
