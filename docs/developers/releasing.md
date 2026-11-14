# Release Prysm

One path for every release: Release Please proposes the version and the
changelog, merging its pull request creates the tag, and
[release.yml](../../.github/workflows/release.yml) builds, checks and publishes
the packages for that tag.

## The version is one file

The product version is the [`VERSION`](../../VERSION) file and nothing else.
Release Please (release-type `simple`) owns it and changes it in its release
pull request, together with `CHANGELOG.md`, `.release-please-manifest.json` and
the one pinned copy for users: the image tag in
[compose.truenas.yaml](../../packaging/docker/compose.truenas.yaml)
(`# x-release-please-version`).

Everything else derives from it:

- The binaries compile it in through
  [`prysm-version`](../../crates/version/src/lib.rs) (`include_str!`, so Cargo
  rebuilds when it changes): `--version`, `prysm_build_info` in `/metrics`, and
  the Luna manifests (`major.minor.patch`).
- The packages read it: [build-deb.sh](../../packaging/debian/build-deb.sh)
  (`-` becomes `~`: `1.0.0-alpha.0` → `1.0.0~alpha.0-1`, which sorts before
  `1.0.0`) and [ipk.py](../../packaging/webos/ipk.py) (`appinfo.json` and the
  IPK file names take only `major.minor.patch`, because `ares-package` accepts
  nothing else).

Nothing else carries it. The crates are `0.0.0` (`[workspace.package]`;
they are never published), so `Cargo.toml` and `Cargo.lock` never change for a
release. The OpenAPI documents' `info.version` is their API's own version
(`1`), so generated contracts never change either.
[scripts/check-release.py](../../scripts/check-release.py) checks this: VERSION
is a semantic version, the crates are `0.0.0`, no Rust source uses
`CARGO_PKG_VERSION`, and the TrueNAS pin equals VERSION; given a tag, also that
the tag is `v` plus VERSION and the manifest has it. Its tests run in CI.

`.release-please-manifest.json` is Release Please's record of the last
release. After 1.0.0, VERSION and the manifest both hold the last released
version between releases. Until then VERSION is `1.0.0-alpha.0`, the version
being built, while the manifest stays at `0.2.0`, the last release of the
earlier implementation (tag `v0.2.0`): Release Please counts commits from that
release, and its first release pull request sets both to `1.0.0`.

## Cut a release

1. Merge work to `main` with [Conventional Commit](guidelines.md#commits-and-pull-requests)
   titles. [release-please.yml](../../.github/workflows/release-please.yml)
   keeps a release pull request open with the next version and the changelog:
   `fix:` bumps the patch, `feat:` the minor, a breaking change (`!`) the major.
   To choose the version yourself, put `Release-As: 1.2.3` in the body of a
   commit on `main`. **The first 1.x release:** from the 0.2.0 manifest, a
   breaking squash title such as `feat!: …` already gives 1.0.0; also put
   `Release-As: 1.0.0` in the squash-merge body, so the version does not
   depend on the title.
2. Review and merge the release pull request. Release Please then creates the
   tag `v<version>` and a draft GitHub release with the changelog.
3. The tag starts [release.yml](../../.github/workflows/release.yml):
   - `prepare`: the tag's commit must be on `main`, and `check-release.py`
     must pass for the tag;
   - `build`: [ci.yml](../../.github/workflows/ci.yml) at that commit with
     `release: true`, all jobs, including the image and Debian package on
     amd64 and arm64 (smoke test, install and purge) and both IPKs;
   - `publish`: [publish.sh](../../packaging/release/publish.sh) pushes
     `ghcr.io/<owner>/prysm-headless:<version>-amd64` and `-arm64` and the
     multi-arch `:<version>`, checks that each image is from the tag's commit,
     attaches `prysm-headless_<deb version>_{amd64,arm64}.deb`,
     `io.prysm.tv-full_<x.y.z>_all.ipk`, `io.prysm.tv-slim_<x.y.z>_all.ipk`,
     `docker-image.txt` (the index digest) and `SHA256SUMS`, and publishes the
     release.

A version with a suffix (`-rc.1`) is published as a pre-release. A stable
release becomes "Latest", and the floating `:latest` image tag points at its
image, only if it is the newest stable release; recovering an older one moves
neither. Whether to keep `:latest` is not decided: it is one line in
publish.sh, marked there.

Packages from one release form a supported desktop/TV pair: the slim TV and
the desktop receiver speak one wire version.

## Recover a failed release

Run **Release** (`workflow_dispatch`) with the existing tag. It rebuilds from
the tag's commit if the release is not yet published; images already pushed
are checked against the commit instead of pushed again. For a release that is
already published it only checks the images and the "Latest" marker. The
publish script comes from the workflow's own commit, so a fix on `main` can
recover an older tag.

## Repository settings

- Release Please uses a GitHub App token: variable `RELEASE_APP_ID` and secret
  `RELEASE_APP_PRIVATE_KEY`. Tags created with the default `GITHUB_TOKEN`
  would not start release.yml.
- release.yml pushes to GHCR with `GITHUB_TOKEN` (`packages: write`). The
  first push creates the `prysm-headless` package; make it public and link it
  to this repository in its package settings.
