#!/bin/sh
# Assembles prysm-headless_<version>_<arch>.deb from a built binary with
# dpkg-deb. Run on Debian/Ubuntu with dpkg-dev, on the binary's own
# architecture (dpkg-shlibdeps reads the system's library info), e.g. through
# the Dockerfile's `deb` target. Usage: build-deb.sh BINARY OUT_DIR
set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 BINARY OUT_DIR" >&2
    exit 2
fi
binary=$1
out=$2
here=$(cd "$(dirname "$0")" && pwd)
v1=$(cd "$here/../.." && pwd)
package=prysm-headless

# Workspace version, e.g. 1.0.0-alpha.0 -> 1.0.0~alpha.0-1 (sorts before
# 1.0.0; "-1" is the packaging revision).
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$v1/Cargo.toml")
[ -n "$version" ] || { echo "no [workspace.package] version in $v1/Cargo.toml" >&2; exit 1; }
version=$(printf '%s' "$version" | tr - '~')-${DEB_REVISION:-1}
arch=$(dpkg --print-architecture)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
root=$work/root

# Runtime library dependencies of the binary.
mkdir -p "$work/shlibs/debian"
printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$package" "$package" \
    >"$work/shlibs/debian/control"
cp "$binary" "$work/shlibs/$package"
shlibs=$(cd "$work/shlibs" && dpkg-shlibdeps -O -e"$package" 2>/dev/null \
    | sed -n 's/^shlibs:Depends=//p')
[ -n "$shlibs" ] || { echo "dpkg-shlibdeps found no dependencies for $binary" >&2; exit 1; }

install -D -m 0755 "$binary" "$root/usr/bin/$package"
install -D -m 0644 "$here/$package.service" "$root/usr/lib/systemd/system/$package.service"
install -D -m 0644 "$here/$package.sysusers" "$root/usr/lib/sysusers.d/$package.conf"
install -D -m 0644 "$v1/hosts/headless/settings.example.json" \
    "$root/usr/share/doc/$package/examples/settings.example.json"
{
    printf 'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n'
    printf 'Upstream-Name: prysm\nSource: https://github.com/ericschaal/prysm\n\n'
    printf 'Files: *\nCopyright: 2025-2026 Prysm contributors\nLicense: Apache-2.0\n'
    printf ' On Debian systems, the full text of the Apache License, Version 2.0 can be\n'
    printf ' found in /usr/share/common-licenses/Apache-2.0.\n'
} >"$work/copyright"
install -D -m 0644 "$work/copyright" "$root/usr/share/doc/$package/copyright"
# Outside the pipeline below so that a failing date stops the script (no pipefail in sh).
changelog_date=$(date -R -u ${SOURCE_DATE_EPOCH:+-d "@$SOURCE_DATE_EPOCH"})
printf '%s (%s) unstable; urgency=medium\n\n  * Release %s.\n\n -- Eric Schaal <eric@schaal.dev>  %s\n' \
    "$package" "$version" "$version" "$changelog_date" \
    | gzip -9n >"$work/changelog.Debian.gz"
install -m 0644 "$work/changelog.Debian.gz" "$root/usr/share/doc/$package/changelog.Debian.gz"

mkdir -p "$root/DEBIAN"
for script in postinst prerm postrm; do
    install -m 0755 "$here/$script" "$root/DEBIAN/$script"
done
size=$(du -sk --exclude=DEBIAN "$root" | cut -f1)
cat >"$root/DEBIAN/control" <<EOF
Package: $package
Version: $version
Architecture: $arch
Maintainer: Eric Schaal <eric@schaal.dev>
Installed-Size: $size
Depends: $shlibs, systemd
Section: video
Priority: optional
Homepage: https://github.com/ericschaal/prysm
Description: Prysm ambient lighting, v1 headless host
 Captures video from a V4L2 device or a capture-only TV and drives WLED
 LEDs from its edges. A web portal on port 8787 configures and controls it.
 Runs as the prysm-headless systemd service with settings in
 /var/lib/prysm/settings.json.
EOF

mkdir -p "$out"
dpkg-deb --root-owner-group -Zxz --build "$root" "$out/${package}_${version}_${arch}.deb"
