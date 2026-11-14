#!/usr/bin/env bash
# Publishes one release from the artifacts of a CI run with `release: true`:
# pushes ghcr.io/<owner>/prysm-headless:<version> for amd64 and arm64 and the
# multi-arch index, checks that every image carries the release commit, then
# attaches both .debs, both TV packages, docker-image.txt and SHA256SUMS to the
# GitHub release and publishes it. Called by .github/workflows/release.yml.
#
# Safe to run again for the same tag: images already pushed are checked, not
# rebuilt, and a release already published only gets the checks. A stable
# release becomes "Latest" (and `:latest`) only if it is the newest stable one.
#
# Environment: GITHUB_REPOSITORY (owner/name), RELEASE_TAG (v1.2.3),
# RELEASE_VERSION (1.2.3), RELEASE_SHA (the tag's commit), RELEASE_DIRECTORY
# (the downloaded artifacts: prysm-headless-<arch>.tar, prysm-headless_*.deb,
# io.prysm.tv-*.ipk), GH_TOKEN; docker is logged in to ghcr.io.
set -euo pipefail

: "${GITHUB_REPOSITORY:?}" "${RELEASE_TAG:?}" "${RELEASE_VERSION:?}" "${RELEASE_SHA:?}" "${RELEASE_DIRECTORY:?}"
export GH_REPO="$GITHUB_REPOSITORY"
owner=$(printf '%s' "${GITHUB_REPOSITORY%%/*}" | tr '[:upper:]' '[:lower:]')
image="ghcr.io/$owner/prysm-headless"
version=$RELEASE_VERSION
# As packaging/debian/build-deb.sh and packaging/webos/ipk.py name them.
deb_version=$(printf '%s' "$version" | tr - '~')
app_version=${version%%[-+]*}
case "$version" in *-*) prerelease=true ;; *) prerelease=false ;; esac

fail() {
  echo "publish: $*" >&2
  exit 1
}

cd "$RELEASE_DIRECTORY"

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
  # The packages this version must consist of, and nothing else.
  shopt -s nullglob
  debs=(./*.deb)
  ipks=(./*.ipk)
  [ "${#debs[@]}" -eq 2 ] || fail "expected 2 .deb files, found ${#debs[@]}"
  [ "${#ipks[@]}" -eq 2 ] || fail "expected 2 .ipk files, found ${#ipks[@]}"
  for flavour in full slim; do
    [ -f "io.prysm.tv-${flavour}_${app_version}_all.ipk" ] || fail "no io.prysm.tv-${flavour}_${app_version}_all.ipk"
  done
  architectures=()
  for deb in "${debs[@]}"; do
    [ "$(dpkg-deb --field "$deb" Package)" = prysm-headless ] || fail "$deb is not prysm-headless"
    packaged=$(dpkg-deb --field "$deb" Version)
    [ "${packaged%-*}" = "$deb_version" ] || fail "$deb has version $packaged, not $deb_version-*"
    architectures+=("$(dpkg-deb --field "$deb" Architecture)")
  done
  [ "$(printf '%s\n' "${architectures[@]}" | sort | tr '\n' ' ')" = 'amd64 arm64 ' ] || fail "the .debs are for ${architectures[*]}"

  for arch in amd64 arm64; do
    target="$image:$version-$arch"
    if docker pull --platform "linux/$arch" "$target" > pull.log 2>&1; then
      [ "$(revision_of "$target")" = "$RELEASE_SHA" ] || fail "$target is not from $RELEASE_SHA"
    elif grep -Eqi 'manifest unknown|name unknown|not found' pull.log; then
      docker image load --input "prysm-headless-$arch.tar"
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
docker buildx imagetools inspect --raw "$image:$version" > manifest.json
python3 - "$image" "$RELEASE_SHA" <<'PY'
import json
import subprocess
import sys

image, sha = sys.argv[1:]
manifests = json.load(open('manifest.json'))['manifests']
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
  shasum -a 256 ./*.deb ./*.ipk docker-image.txt > SHA256SUMS
  if [ "$release" = missing ]; then
    gh release create "$RELEASE_TAG" --verify-tag --draft --title "Prysm $version" --generate-notes
  fi
  gh release upload "$RELEASE_TAG" --clobber ./*.deb ./*.ipk docker-image.txt SHA256SUMS
  gh release edit "$RELEASE_TAG" --draft=false --prerelease="$prerelease" --latest=false
fi

# Only the newest stable release is "Latest"; recovering an older one, or a
# pre-release, leaves it and `:latest` where they are.
if [ "$prerelease" = false ]; then
  gh api --paginate "repos/$GITHUB_REPOSITORY/releases" \
    --jq '.[] | select(.draft == false and .prerelease == false) | .tag_name' > releases.txt
  newest=$(python3 - <<'PY'
import re
tags = [t for t in open('releases.txt').read().split() if re.fullmatch(r'v\d+\.\d+\.\d+', t)]
print(max(tags, key=lambda t: tuple(map(int, t[1:].split('.')))) if tags else '')
PY
)
  if [ "$newest" = "$RELEASE_TAG" ]; then
    gh release edit "$RELEASE_TAG" --latest
    # The floating `:latest` image tag (not yet decided): delete this line to
    # publish only `:<version>`; its tests are the `latest` asserts in
    # scripts/tests/test_release.py.
    docker buildx imagetools create --tag "$image:latest" "$image@$digest"
  fi
fi
