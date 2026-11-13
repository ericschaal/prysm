# Contribution guidelines

## Commits and pull requests

Use Conventional Commits for commit messages and PR titles:
`feat:`, `fix:`, `perf:`, `docs:`, `refactor:`, `test:`, `build:`, `ci:`,
`chore:`, `style:`, or `revert:`. Add a scope when useful, such as
`fix(v1): keep the receiver port after a failed save`. Mark breaking changes
with `!`. Squash-merge PRs using their title as the commit title.

Releases: [releasing](releasing.md).

## Code

- Concurrency lives in `prysm-runtime`. An adapter is one plain blocking
  object behind one trait: no channels, `select!`, identities or lifecycle
  state machines ([decision](decisions/2026-11-12-one-runtime-plain-adapters.md)).
- Keep application, serialisation and runtime code out of `prysm-core`.
- Preserve typed validation (`deny_unknown_fields`, ranges, field paths) and
  [error causes](error-handling.md).
- Keep engine commands ordered; let frames be replaced, never queued.
- Preserve the session's [shutdown order](architecture.md#shutdown-ordering)
  (stop, close source, idle output, join) and output idle compensation on
  every exit. Keep native watchdogs inside capture adapters.
- Keep process options on the command line and run-time settings in the
  settings file.
- Rust owns contracts. After a settings, state or route change, run
  `npm --prefix v1/frontend run codegen` and update the
  [portal contract](../../v1/surfaces/web-portal/HTTP-CONTRACT.md).
- Change callers, fixtures and docs together. Test the changed behaviour.
- TV tests on hardware keep isolation (probe identity, private directory),
  timeouts and cleanup. Builds do not deploy.

## Documentation

[Users](../users/README.md): steps to working lights.
[Developers](README.md): current architecture, commands and contracts.
[Research](../../research/README.md): experiments and measurements.
Reference detail lives next to the code (`v1/**/README.md`, the HTTP
contract); guides link to it instead of copying it.

Write short, task-first pages: prerequisites, exact commands or clicks,
expected result. Use plain language and British spelling in prose ("colour",
as the portal does); keep code identifiers as they are. Cut promotion, vague
steps and implementation narration in user guides.

Say plainly when a step has not been verified on hardware. Never claim a test
that has not run. Do not invent version numbers, URLs or features.

Give each topic one home; link instead of repeating it. Keep public paths and
anchors working. Link only committed files, never ignored build, `work/` or
temporary files.

```sh
python3 -m unittest discover -s scripts/tests
python3 scripts/check-doc-links.py
```

[CI](../../.github/workflows/documentation.yml) checks repository links on
every PR. Public URLs are checked weekly and on manual runs; external
availability does not block merging.
