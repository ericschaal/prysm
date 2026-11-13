# Release Prysm

Prysm 1.x is released by [v1-release.yml](../../.github/workflows/v1-release.yml)
when a `v1.*` tag is pushed.

## Cut a release

1. Set `version` under `[workspace.package]` in
   [`v1/Cargo.toml`](../../v1/Cargo.toml) and update `v1/Cargo.lock`
   in the same change. It is the only version source: binaries, the Debian
   package and the IPKs all derive from it.
2. Merge to `main`.
3. Tag that commit with `v` plus the version, for example `v1.0.0-alpha.0`,
   and push the tag.

The workflow then:

- runs the v1 gate ([v1.yml](../../.github/workflows/v1.yml)), which also
  builds both IPKs with the pinned webOS SDK;
- builds the image and Debian package on amd64 and arm64, smoke-tests the
  image and installs the package;
- checks that the tag equals `v` plus the workspace version;
- pushes `ghcr.io/<owner>/prysm-headless:<version>` (multi-arch);
- attaches the `.deb` files and both IPKs, renamed
  `io.prysm.tv-full_<version>_all.ipk` and `io.prysm.tv-slim_<version>_all.ipk`,
  to a GitHub release.

A version with a suffix (`-alpha.0`) becomes a pre-release. v1 releases are
never marked "Latest". A manual run (`workflow_dispatch`) builds and tests
everything and keeps the artifacts for seven days, without publishing.

Packages from one release form a supported desktop/TV pair: the slim TV and
the desktop receiver speak one wire version.

## Commit conventions

PR titles follow [Conventional Commits](guidelines.md#commits-and-pull-requests)
and are checked in CI.

## Pre-v1 release automation

Release Please ([release-please.yml](../../.github/workflows/release-please.yml),
[release.yml](../../.github/workflows/release.yml)) and the root `VERSION` file
still release the earlier implementation on other `v*` tags and ignore
`v1.*` tags. They do not build anything in `v1/`.
