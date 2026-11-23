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
  [`prysm-version`](../../crates/engine/version/src/lib.rs) (`include_str!`, so Cargo
  rebuilds when it changes): `--version`, the portal's `State.version`,
  `prysm_build_info` in `/metrics`, and the Luna manifests
  (`major.minor.patch`).
- The packages read it: [build-deb.sh](../../packaging/debian/build-deb.sh)
  (`-` becomes `~`: `1.0.0-rc.1` → `1.0.0~rc.1-1`, which sorts before
  `1.0.0`) and [ipk.py](../../packaging/webos/ipk.py) (`appinfo.json` and the
  IPK file names take only `major.minor.patch`, because `ares-package` accepts
  nothing else).

Nothing else carries it. The crates are `0.0.0` (`[workspace.package]`;
they are never published), so `Cargo.toml` and `Cargo.lock` never change for a
release. The OpenAPI documents' `info.version` is their API's own version
(the HTTP documents' `1`, Luna's `4`), and the exported `State` fixtures
carry a fixed `version`, so generated contracts never change for a release.
[scripts/check-release.py](../../scripts/check-release.py) checks this: VERSION
is a semantic version without build metadata (an image tag cannot hold
`+`), the crates are `0.0.0`, no Rust source uses
`CARGO_PKG_VERSION`, and the TrueNAS pin equals VERSION; given a tag, also that
the tag is `v` plus VERSION and the manifest has it. Its tests run in CI.

`.release-please-manifest.json` is Release Please's record of the last
release. Between releases, VERSION and the manifest both hold the last
released version (since `1.0.0-rc.1`), and Release Please counts commits from
its tag.

## What the version promises

From 1.0.0 the version follows [semantic versioning](https://semver.org/) for
these surfaces. A change that breaks a user or client of the previous release
is a major release (`!`); an addition is a minor one; a fix is a patch.

Covered:

- the portal HTTP API under `/api/v1` and its values, `State` included
  ([contract](../../crates/surfaces/web-portal/HTTP-CONTRACT.md)): routes,
  fields, value sets, status codes and failure kinds. A new field is a
  minor change; clients must ignore fields they do not know. Enum values that
  Prysm sends (HTTP responses and events, `State`, the Luna service's replies,
  the capture stream's messages) are open: a minor release may add a value.
  A client must accept a value it does not know and read it as the contract
  states for that field (never as off or stopped), and must send a value it
  read back unchanged; the one exception is a tag that selects a whole shape
  (Luna `flavor`), where the stated reading is "this state cannot be shown".
  Values that Prysm receives (request bodies, the settings file, Luna
  requests) are closed: Prysm refuses one it does not know (`400`, or `422`
  with `field`; a Luna error; a settings error), so a new value there is a minor
  change that only newer clients send
  ([decision](decisions/2026-11-22-open-response-enums.md));
- the settings file and its `format` number
  ([settings-file](../../crates/adapters/settings-file/README.md)). A Prysm
  reads files of its own format only and refuses any other number, naming
  both; a file it refuses starts the host unconfigured rather than failing
  it, and is kept beside the new file at the next save. The number is
  raised only when Prysm can no longer read files with the previous one (a
  field renamed or removed, or its meaning changed),
  which is a major change. Additions keep the number and are minor: an older
  Prysm refuses a newer file by the unknown field's name;
- the TV parts one installed version uses of another: the Luna service
  `io.github.ericschaal.prysm.service` with `getState` and `uninstall` (an update calls the
  installed service's), and the supervisor's `setup` command, which an older
  boot hook runs;
- the capture stream between a slim TV and a desktop Prysm, by its protocol
  version (`prysm-capture.v6`, `CAPTURE_VERSION`). The number is raised only
  when a peer of the previous number can no longer work with this one (a
  field removed or its meaning changed, the frame header changed, or a new
  message the peer must not ignore), which is a major change. Additions keep
  the number and are minor (fields, message types, flag bits, encodings,
  offer keys, close codes): both ends ignore what they do not know, and
  neither sends what the other must understand unless it asked for it (the
  receiver in its request, the sender in its offer). A Prysm refuses a TV of
  another number with `426`, listing the numbers it speaks; the TV names
  both versions and the TV app asks to update the older of the two
  ([capture-remote](../../crates/adapters/capture-remote/README.md#wire-protocol));
- the hosts' command-line flags and the exit codes
  [hosts](workspace.md#hosts) documents: 0 after a clean stop, 1 on failure,
  2 for bad arguments, 101 if a panic ended the host, 130 when the host
  exited at once on a second SIGINT or SIGTERM; `--check-settings` 0 when
  valid, else 1;
- package and image names: the Debian package `prysm-headless` and its
  options file `/etc/default/prysm-headless` (`OPTIONS`), the image
  `ghcr.io/<owner>/prysm-headless`, the IPKs
  `io.github.ericschaal.prysm-full` and `io.github.ericschaal.prysm-slim`;
- the image tags: `:X.Y.Z` for every release, never moved; `:X`, the newest
  stable release of major version X; `:latest`, the newest stable release.
  A pre-release (`X.Y.Z-rc.N`) is tagged `:X.Y.Z-rc.N` only. Every promised
  tag is the multi-arch index (amd64, arm64).

Not covered: the Rust crates (`0.0.0`, never published) and their APIs, the
Luna methods only the same package's TV app calls (`setPreferences`,
`setEnabled`, `setTarget`), the worker link between the supervisor and its
worker (both ship in one IPK, and it stays strict), `/metrics` names, the
terminal UI, log and message wording,
the `research/` and acceptance tooling, and behaviour that is not in a
contract (timings, colours produced).

## Cut a release

1. Merge work to `main` with [Conventional Commit](guidelines.md#commits-and-pull-requests)
   titles. [release-please.yml](../../.github/workflows/release-please.yml)
   keeps a release pull request open with the next version and the changelog:
   `fix:` bumps the patch, `feat:` the minor, a breaking change (`!`) the major.
   **To choose the version yourself**, put this block in the description of a
   pull request before you squash-merge it (the description becomes the
   commit body, and Release Please reads the block instead of it):

   ```text
   BEGIN_COMMIT_OVERRIDE
   docs: what the pull request does

   Release-As: 1.2.3
   END_COMMIT_OVERRIDE
   ```

   A bare `Release-As:` line in a description is not reliable: after a
   `BREAKING CHANGE:` note it is read as part of that note and ignored. The
   block also works afterwards: add it to the merged pull request's
   description and run the workflow again.

   **Release candidates.** A version with a suffix (`1.0.0-rc.1`) is published
   as a pre-release. [release-please-config.json](../../release-please-config.json)
   keeps Release Please's `prerelease` versioning (`prerelease-type: rc`), and
   one setting there chooses what the release pull request proposes:
   - `"prerelease": true`: candidates. After `1.0.0-rc.2` it proposes
     `1.0.0-rc.3`, whatever the commit types. From a stable version the first
     candidate has no number (`1.1.0-rc`, then `1.1.0-rc.1`).
   - `"prerelease": false`: final versions. After `1.0.0-rc.3` it proposes
     `1.0.0`; after `1.0.0`, `1.0.1` or `1.1.0` as usual.

   Release Please has no step that turns a published candidate into the final
   version: the final version is the next release, made by a pull request
   that sets `"prerelease": false`. Check the version in the release pull
   request's title before merging it.
2. Review and merge the release pull request. Release Please then creates the
   tag `v<version>` and a draft GitHub release with the changelog.
3. The tag starts [release.yml](../../.github/workflows/release.yml):
   - `prepare`: the tag's commit must be on `main`, and `check-release.py`
     must pass for the tag;
   - `build`: [ci.yml](../../.github/workflows/ci.yml) at that commit with
     `release: true`, all jobs. The `docker` job builds the image, binary and
     Debian package on amd64 and arm64 with `cargo xtask dist` (smoke test,
     install and purge), and `webos-ipk` both IPKs; each keeps its `dist/`
     and `manifest.json` as one artifact
     ([packaging](packaging.md#artifacts-in-dist));
   - `publish`: [publish.sh](../../packaging/release/publish.sh) checks every
     file against its manifest with `cargo xtask release-artifacts` (the
     release's version and commit, a clean tree, the SHA-256, a plain file
     name in one artifact only and not yet in the release directory;
     [release.rs](../../crates/tools/xtask/src/release.rs)),
     built from the workflow's own checkout of the tools, pushes
     `ghcr.io/<owner>/prysm-headless:<version>-amd64` and `-arm64` (staging
     tags for the index, not promised) and the multi-arch `:<version>`,
     checks that each image is from the tag's commit,
     attaches the packages under the names users download,
     `prysm-headless_<version>_{amd64,arm64}.deb`,
     `io.github.ericschaal.prysm-full_<x.y.z>_all.ipk` and `io.github.ericschaal.prysm-slim_<x.y.z>_all.ipk`,
     with `docker-image.txt` (the index digest) and `SHA256SUMS`, and
     publishes the release.

A version with a suffix (`-rc.1`) is published as a pre-release, with its
exact image tag only. A stable release moves the image tag `:X` of its major
version X if it is the newest stable release of that major, and becomes
"Latest" with the image tag `:latest` if it is the newest stable release of
all. So a patch of an older major (`1.4.1` after `2.0.0`) moves `:1` only, and
recovering an older release moves neither. The tests in
[test_release.py](../../scripts/tests/test_release.py) hold the table.

Packages from one release form a supported desktop/TV pair: the slim TV and
the desktop receiver speak one wire version.

## Recover a failed release

Run **Release** (`workflow_dispatch`) with the existing tag. It rebuilds from
the tag's commit if the release is not yet published; images already pushed
are checked against the commit instead of pushed again. For a release that is
already published it only checks the images, and sets `:X`, `:latest` and
the "Latest" marker by the rule above. The
publish script comes from the workflow's own commit, so a fix on `main` can
recover an older tag.

## Repository settings

- Release Please uses a GitHub App token: variable `RELEASE_APP_ID` and secret
  `RELEASE_APP_PRIVATE_KEY`. Tags created with the default `GITHUB_TOKEN`
  would not start release.yml.
- release.yml pushes to GHCR with `GITHUB_TOKEN` (`packages: write`). The
  first push creates the `prysm-headless` package; make it public and link it
  to this repository in its package settings.
