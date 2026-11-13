#!/bin/sh
# Install a Prysm TV package (full or slim) on a rooted TV over SSH.
#
#   install-ipk.sh [--dry-run] root@TV io.prysm.tv-full_1.0.0_all.ipk
#
# The file is a release asset or build-ipk.sh's output,
# io.prysm.tv-full_<version>_all.ipk or io.prysm.tv-slim_<version>_all.ipk;
# both install as the app io.prysm.tv.
#
# `ares-install` fails with "Unable to exec" on webOS 10, so this copies the
# IPK to /media/developer/temp/ and installs it through
# com.webos.appInstallService/dev/install, then runs the package's setup.sh
# (Luna registration, rescan, activation): after an install the service is
# not running until setup runs again. The service resumes the lighting it had
# before the update (the intent is kept per boot in /var/lib/io.prysm.tv).
#
# luna-send waits for replies only while its stdin is open, so every call
# keeps it open with `(sleep N) | ssh ...` and bounds it with `timeout`.
# --dry-run prints the commands and touches nothing. Builds never call this.
set -eu

APP=io.prysm.tv
TEMP=/media/developer/temp
SETUP=/media/developer/apps/usr/palm/services/$APP.service/setup.sh
INSTALL_TIMEOUT=120

dry_run=false
if [ "${1:-}" = --dry-run ]; then
    dry_run=true
    shift
fi
if [ $# -ne 2 ]; then
    echo "usage: $0 [--dry-run] root@TV path/to/$APP-full_VERSION_all.ipk (or -slim_)" >&2
    exit 2
fi
host=$1
ipk=$2
name=$(basename "$ipk")
case $name in
    "$APP"-full_*_all.ipk | "$APP"-slim_*_all.ipk) ;;
    *) echo "$0: $name is not a Prysm TV package ($APP-full_… or $APP-slim_…)" >&2; exit 2 ;;
esac
case $name in
    *[!A-Za-z0-9._-]*) echo "$0: unexpected characters in $name" >&2; exit 2 ;;
esac
if [ ! -f "$ipk" ]; then
    echo "$0: no such file: $ipk" >&2
    exit 2
fi
remote=$TEMP/$name
payload="{\"id\":\"$APP\",\"ipkUrl\":\"$remote\",\"subscribe\":true}"

# On the TV: luna-send in the background with the held stdin (fd 3: an
# asynchronous command would otherwise read /dev/null), its replies polled
# once a second until the installed state or an error reply.
install="exec 3<&0
out=\$(mktemp)
timeout $INSTALL_TIMEOUT luna-send -i luna://com.webos.appInstallService/dev/install '$payload' <&3 >\"\$out\" 2>&1 &
pid=\$!
result=1
i=0
while [ \$i -lt $INSTALL_TIMEOUT ]; do
    if grep -qE '\"returnValue\":false|failed|FAILED' \"\$out\"; then break; fi
    if grep -q '\"installed\"' \"\$out\"; then result=0; break; fi
    kill -0 \$pid 2>/dev/null || break
    sleep 1
    i=\$((i + 1))
done
kill \$pid 2>/dev/null
cat \"\$out\"
rm -f \"\$out\"
exit \$result"

run() {
    echo "+ $*" >&2
    if [ "$dry_run" = false ]; then
        "$@"
    fi
}

copy() {
    echo "+ ssh $host 'mkdir -p $TEMP && cat > $remote' < $ipk" >&2
    if [ "$dry_run" = false ]; then
        ssh "$host" "mkdir -p $TEMP && cat > $remote" < "$ipk"
    fi
}

held() {
    seconds=$1
    shift
    echo "+ (sleep $((seconds + 2))) | ssh $host \"$*\"" >&2
    if [ "$dry_run" = false ]; then
        # A named pipe instead of a pipeline, so the call returns when ssh
        # does and not when the sleep ends.
        pipe=$(mktemp -u)
        mkfifo -m 600 "$pipe"
        sleep $((seconds + 2)) > "$pipe" &
        holder=$!
        result=0
        ssh "$host" "$*" < "$pipe" || result=$?
        kill "$holder" 2>/dev/null
        wait "$holder" 2>/dev/null
        rm -f "$pipe"
        return "$result"
    fi
}

copy
status=0
held "$INSTALL_TIMEOUT" "$install" || status=$?
run ssh "$host" "rm -f $remote"
if [ "$status" -ne 0 ]; then
    echo "$0: installation failed (see the replies above)" >&2
    exit 1
fi
run ssh "$host" "$SETUP"
held 12 "timeout 12 luna-send -n 1 luna://$APP.service/getState '{}'"
