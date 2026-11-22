#!/usr/bin/env bash
# Publishes one release from the artifacts of a CI run with `release: true`:
# pushes ghcr.io/<owner>/prysm-headless:<version> for amd64 and arm64 as the
# multi-arch index, checks that every image carries the release commit, moves
# `:<major>` and `:latest` to it when it is the newest stable release, attaches
# both .debs, both TV packages, docker-image.txt and SHA256SUMS to the GitHub
# release and publishes it. Called by .github/workflows/release.yml.
#
# The promised image tags are `:<version>` (never moved), `:<major>` (the newest
# stable release of that major version) and `:latest` (the newest stable
# release); a pre-release gets `:<version>` only. The per-architecture tags
# `:<version>-amd64` and `-arm64` only stage the images for the index and are
# not promised.
#
# Safe to run again for the same tag: images already pushed are checked, not
# rebuilt, and a release already published only gets the checks and the
# floating tags.
#
# Environment: GITHUB_REPOSITORY (owner/name), RELEASE_TAG (v1.2.3),
# RELEASE_VERSION (1.2.3), RELEASE_SHA (the tag's commit), RELEASE_DIRECTORY,
# GH_TOKEN; docker is logged in to ghcr.io; cargo runs the Rust that this
# checkout's rust-toolchain.toml pins. RELEASE_DIRECTORY holds one directory
# per CI artifact, each a `cargo xtask dist` output: files under their dist/
# names and the manifest.json that records them. `cargo xtask
# release-artifacts`, built from this script's checkout, checks every file
# against its manifest entry (this version and commit, a clean tree, the
# SHA-256) before anything is published. The release attaches
# prysm-headless-linux-<arch>.deb as prysm-headless_<version>_<arch>.deb (the
# product version, not Debian's: GitHub replaces the `~` of a Debian
# pre-release version in an asset's name, and SHA256SUMS would then name files
# that are not there) and prysm-tv-<flavour>.ipk as
# <app>-<flavour>_<x.y.z>_all.ipk.
set -euo pipefail

: "${GITHUB_REPOSITORY:?}" "${RELEASE_TAG:?}" "${RELEASE_VERSION:?}" "${RELEASE_SHA:?}" "${RELEASE_DIRECTORY:?}"
export GH_REPO="$GITHUB_REPOSITORY"
owner=$(printf '%s' "${GITHUB_REPOSITORY%%/*}" | tr '[:upper:]' '[:lower:]')
image="ghcr.io/$owner/prysm-headless"
version=$RELEASE_VERSION
# As packaging/debian/build-deb.sh and packaging/webos/ipk.py name them.
deb_version=$(printf '%s' "$version" | tr - '~')
app_version=${version%%[-+]*}
# The TV app id, as packaging/webos/build-ipk.sh names its packages.
app=io.github.ericschaal.prysm
case "$version" in *-*) prerelease=true ;; *) prerelease=false ;; esac

fail() {
  echo "publish: $*" >&2
  exit 1
}

# This script's checkout, which builds xtask with its own toolchain and lock.
tools=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$RELEASE_DIRECTORY"
release_directory=$PWD

if gh release view "$RELEASE_TAG" --json isDraft > release.json 2> release-error; then
  if python3 -c 'import json; raise SystemExit(json.load(open("release.json"))["isDraft"])'; then
    release=published
  else
    release=draft
  fi
elif grep -qi 'release not found' release-error; then
  release=missing
else
  cat release-error >&2
  exit 1
fi

revision_of() {
  docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$1"
}

if [ "$release" != published ]; then
  # Moves every artifact here, after checking it against its manifest entry.
  # From the tools' checkout, so rustup reads its rust-toolchain.toml.
  (cd "$tools" && cargo run --quiet --locked -p xtask -- release-artifacts \
    "$release_directory" --version "$version" --commit "$RELEASE_SHA")

  # The packages this version consists of, under the names users download.
  assets=()
  for arch in amd64 arm64; do
    deb=prysm-headless-linux-$arch.deb
    [ -f "$deb" ] || fail "no $deb"
    [ -f "prysm-headless-linux-$arch.tar" ] || fail "no prysm-headless-linux-$arch.tar"
    [ "$(dpkg-deb --field "$deb" Package)" = prysm-headless ] || fail "$deb is not prysm-headless"
    [ "$(dpkg-deb --field "$deb" Architecture)" = "$arch" ] || fail "$deb is not for $arch"
    packaged=$(dpkg-deb --field "$deb" Version)
    [ "${packaged%-*}" = "$deb_version" ] || fail "$deb has version $packaged, not $deb_version-*"
    named=prysm-headless_${version}_${arch}.deb
    mv "$deb" "$named"
    assets+=("$named")
  done
  for flavour in full slim; do
    [ -f "prysm-tv-$flavour.ipk" ] || fail "no prysm-tv-$flavour.ipk"
    named=${app}-${flavour}_${app_version}_all.ipk
    mv "prysm-tv-$flavour.ipk" "$named"
    assets+=("$named")
  done

  for arch in amd64 arm64; do
    target="$image:$version-$arch"
    if docker pull --platform "linux/$arch" "$target" > pull.log 2>&1; then
      [ "$(revision_of "$target")" = "$RELEASE_SHA" ] || fail "$target is not from $RELEASE_SHA"
    elif grep -Eqi 'manifest unknown|name unknown|not found' pull.log; then
      docker image load --input "prysm-headless-linux-$arch.tar"
      [ "$(revision_of prysm-headless:local)" = "$RELEASE_SHA" ] || fail "the $arch image is not from $RELEASE_SHA"
      docker image tag prysm-headless:local "$target"
      docker image push "$target"
    else
      cat pull.log >&2
      exit 1
    fi
  done
  if docker buildx imagetools inspect "$image:$version" > inspect.log 2>&1; then
    cat inspect.log
  elif grep -Eqi 'manifest unknown|name unknown|not found' inspect.log; then
    docker buildx imagetools create --tag "$image:$version" "$image:$version-amd64" "$image:$version-arm64"
  else
    cat inspect.log >&2
    exit 1
  fi
fi

# Whatever the state: the index has both architectures, each from the commit.
docker buildx imagetools inspect --raw "$image:$version" > index.json
python3 - "$image" "$RELEASE_SHA" <<'PY'
import json
import subprocess
import sys

image, sha = sys.argv[1:]
manifests = json.load(open('index.json'))['manifests']
platforms = {m['platform']['architecture']: m['digest'] for m in manifests
             if m['platform'].get('os') == 'linux'}
assert set(platforms) == {'amd64', 'arm64'}, platforms
for arch, digest in sorted(platforms.items()):
    subprocess.run(['docker', 'pull', '--platform', f'linux/{arch}', f'{image}@{digest}'], check=True)
    revision = subprocess.check_output(['docker', 'image', 'inspect', '--format',
        '{{index .Config.Labels "org.opencontainers.image.revision"}}', f'{image}@{digest}'], text=True).strip()
    assert revision == sha, (arch, revision)
PY
digest=$(docker buildx imagetools inspect --format '{{json .Manifest}}' "$image:$version" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["digest"])')
printf '%s@%s\n' "$image" "$digest" > docker-image.txt

if [ "$release" != published ]; then
  shasum -a 256 "${assets[@]}" docker-image.txt > SHA256SUMS
  if [ "$release" = missing ]; then
    gh release create "$RELEASE_TAG" --verify-tag --draft --title "Prysm $version" --generate-notes
  fi
  gh release upload "$RELEASE_TAG" --clobber "${assets[@]}" docker-image.txt SHA256SUMS
  gh release edit "$RELEASE_TAG" --draft=false --prerelease="$prerelease" --latest=false
fi

# The image tags this release moves: `:X` if it is the newest stable release
# of its major version X, and `:latest` (and the release's "Latest" marker) if
# it is the newest stable release of all. A pre-release has its exact tag only,
# and an older release (one recovered, or a patch of an older major) moves only
# the tags it is the newest for.
if [ "$prerelease" = false ]; then
  gh api --paginate "repos/$GITHUB_REPOSITORY/releases" \
    --jq '.[] | select(.draft == false and .prerelease == false) | .tag_name' > releases.txt
  floating=$(python3 - "$version" <<'PY'
import re
import sys

def key(version):
    return tuple(map(int, version.split('.')))

version = key(sys.argv[1])
stable = [key(t[1:]) for t in open('releases.txt').read().split() if re.fullmatch(r'v\d+\.\d+\.\d+', t)]
tags = []
if version == max(v for v in stable + [version] if v[0] == version[0]):
    tags.append(str(version[0]))
if version == max(stable + [version]):
    tags.append('latest')
print(' '.join(tags))
PY
)
  arguments=()
  for tag in $floating; do
    arguments+=(--tag "$image:$tag")
  done
  if [ ${#arguments[@]} -gt 0 ]; then
    docker buildx imagetools create "${arguments[@]}" "$image@$digest"
  fi
  case " $floating " in
  *" latest "*) gh release edit "$RELEASE_TAG" --latest ;;
  esac
fi
