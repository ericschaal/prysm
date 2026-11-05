# Release Prysm

Merge the Release Please PR to release a new version. Review its changelog and
version first; there is no second publication approval.

## Automation

- Pull requests and pushes to `main` run checks and package builds without publishing.
- Pushes to `main` run Release Please, which maintains the release PR.
- Merging that PR creates a `vMAJOR.MINOR.PATCH` tag and draft GitHub Release.
- The tag runs all checks and packaging jobs at that exact commit. Once they pass,
  the publisher uploads the Debian arm64 package, webOS IPK, pinned Compose file,
  Docker digest, and checksums; pushes amd64/arm64 images to GHCR; and publishes
  the release. Only the newest stable release updates the Docker `latest` tag.

`VERSION` is the product version. Release Please updates it, the host manifest
and lockfile, webOS app metadata, Compose image pins, and its own manifest
together. Internal crates are not separately released. While on `0.x`, breaking
changes bump the minor version; compatible fixes bump the patch version.
Only stable three-part versions are supported.

The IPK always includes hardware H.264 support in both binaries. Runtime encoder
selection is separate. Packages from one release form the supported host/TV pair.

## Repository setup

Register a private GitHub App, disable its webhook, and grant repository
**Contents**, **Issues**, and **Pull requests** read/write permissions. Install it
only on this repository. Store its numeric App ID and downloaded private key:

```sh
gh variable set RELEASE_APP_ID --body YOUR_APP_ID
gh secret set RELEASE_APP_PRIVATE_KEY < /path/to/private-key.pem
```

The App creates PRs and tags so their events trigger CI. Publication uses the
job-scoped `GITHUB_TOKEN` with contents/packages write permissions.
Make the GHCR package public after its first push. Enable immutable GitHub
Releases so published tags and assets cannot be replaced.

Use squash merges with the PR title as the commit title. PR titles are checked
against the [Conventional Commit rules](guidelines.md#commits-and-pull-requests).
Actions are pinned to upstream commits; Dependabot proposes weekly updates.
Workspace Rust dependencies use exact versions; update the manifest and lockfile
together.

## Recover a release

Rerun failed jobs in the Release workflow to reuse its tested artifacts. Artifacts
are retained for seven days. To retry an existing tag from scratch:

```sh
gh workflow run release.yml --ref main -f tag=v0.2.0
```

Manual recovery uses publication tooling from the workflow revision. Product
files and all build inputs still come from the original release tag. Merge a
publisher fix before dispatching recovery; the release tag stays in place.

The workflow rejects tag/version mismatches and commits outside `main`. Existing
versioned images must identify the same source commit. Draft assets may be
replaced during recovery; published assets are preserved. A published release
retry only verifies its images and recovers stable-channel promotion.

GHCR and GitHub Releases cannot publish atomically. If either publication fails,
leave the tag in place and rerun; never move it to another commit. After
publication, fixes require a new version. Back up settings before upgrading;
restoring an older binary may also require restoring its settings backup.
