#!/usr/bin/env bash
set -euo pipefail
export GH_REPO="$GITHUB_REPOSITORY"
image="ghcr.io/$(printf '%s' "$GITHUB_REPOSITORY" | tr '[:upper:]' '[:lower:]')"
mkdir -p "$RELEASE_DIRECTORY"
cd "$RELEASE_DIRECTORY"

# Failed-job reruns retain old prepare outputs, so read the live publication state.
if gh release view "$RELEASE_TAG" --json isDraft > release.json 2> release-error; then
  ALREADY_PUBLISHED=$(python3 -c 'import json; print(str(not json.load(open("release.json"))["isDraft"]).lower())')
elif grep -qi 'release not found' release-error; then
  ALREADY_PUBLISHED=false
else
  cat release-error >&2
  exit 1
fi

if [ "$ALREADY_PUBLISHED" != true ]; then
  cp "$GITHUB_WORKSPACE/compose.yaml" .
  test "$(find . -maxdepth 1 -name '*.deb' | wc -l)" -eq 1
  test "$(find . -maxdepth 1 -name '*.ipk' | wc -l)" -eq 1
  test -f "io.prysm.capture_${RELEASE_VERSION}_all.ipk"
  test "$(dpkg-deb --field ./*.deb Package)" = prysm-led
  test "$(dpkg-deb --field ./*.deb Architecture)" = arm64
  deb_version=$(dpkg-deb --field ./*.deb Version)
  test "${deb_version%-*}" = "$RELEASE_VERSION"
  for arch in amd64 arm64; do
    target="$image:$RELEASE_VERSION-$arch"
    if docker pull --platform "linux/$arch" "$target" > pull.log 2>&1; then
      cat pull.log
      test "$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$target")" = "$RELEASE_SHA"
    elif grep -Eqi 'manifest unknown|name unknown|not found' pull.log; then
      docker image load --input "prysm-$arch.tar"
      test "$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' prysm:local)" = "$RELEASE_SHA"
      docker image tag prysm:local "$target"
      docker image push "$target"
    else
      cat pull.log >&2
      exit 1
    fi
  done
  if docker buildx imagetools inspect "$image:$RELEASE_VERSION" > inspect.log 2>&1; then
    cat inspect.log
  elif grep -Eqi 'manifest unknown|name unknown|not found' inspect.log; then
    docker buildx imagetools create --tag "$image:$RELEASE_VERSION" "$image:$RELEASE_VERSION-amd64" "$image:$RELEASE_VERSION-arm64"
  else
    cat inspect.log >&2
    exit 1
  fi
fi

docker buildx imagetools inspect --raw "$image:$RELEASE_VERSION" > manifest.json
python3 - <<'PY'
import json
import os
import subprocess

image = f'ghcr.io/{os.environ["GITHUB_REPOSITORY"].lower()}'
version = os.environ['RELEASE_VERSION']
manifest = json.load(open('manifest.json'))
platforms = {m['platform']['architecture']: m['digest'] for m in manifest['manifests']
             if m['platform']['os'] == 'linux'}
assert set(platforms) == {'amd64', 'arm64'}, platforms
for arch, digest in platforms.items():
    subprocess.run(['docker', 'pull', '--platform', f'linux/{arch}', f'{image}@{digest}'], check=True)
    revision = subprocess.check_output(['docker', 'image', 'inspect', '--format',
        '{{index .Config.Labels "org.opencontainers.image.revision"}}', f'{image}@{digest}'], text=True).strip()
    assert revision == os.environ['RELEASE_SHA'], (arch, revision)
PY
digest=$(docker buildx imagetools inspect --format '{{json .Manifest}}' "$image:$RELEASE_VERSION" | python3 -c 'import json,sys; print(json.load(sys.stdin)["digest"])')
printf '%s@%s\n' "$image" "$digest" > docker-image.txt

if [ "$ALREADY_PUBLISHED" != true ]; then
  shasum -a 256 ./*.deb ./*.ipk compose.yaml docker-image.txt > SHA256SUMS
  if ! gh release view "$RELEASE_TAG" --json isDraft > release.json; then
    gh release create "$RELEASE_TAG" --verify-tag --draft --title "$RELEASE_TAG" --generate-notes
  else
    python3 -c 'import json; assert json.load(open("release.json"))["isDraft"], "Release was already published"'
  fi
  gh release upload "$RELEASE_TAG" ./*.deb ./*.ipk compose.yaml docker-image.txt SHA256SUMS --clobber
  gh release edit "$RELEASE_TAG" --draft=false --latest=false
fi

# A recovery of an older version must not move the stable channel backwards.
gh api --paginate "repos/$GITHUB_REPOSITORY/releases" --jq '.[] | select(.draft == false and .prerelease == false) | .tag_name' > releases.txt
latest=$(python3 - <<'PY'
import re
tags = [tag for tag in open('releases.txt').read().splitlines() if re.fullmatch(r'v\d+\.\d+\.\d+', tag)]
print(max(tags, key=lambda tag: tuple(map(int, tag[1:].split('.')))))
PY
)
if [ "$latest" = "$RELEASE_TAG" ]; then
  docker buildx imagetools create --tag "$image:latest" "$image@$digest"
  gh release edit "$RELEASE_TAG" --latest
fi
