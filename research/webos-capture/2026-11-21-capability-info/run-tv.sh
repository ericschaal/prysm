#!/usr/bin/env bash
# One bounded TV session for the capability probe (not yet run).
#
# Usage: PRYSM_TV=root@<tv-address> run-tv.sh <probe-binary> <local-evidence-dir> [--preprocess]
#
# Before running, turn lighting off in the TV app (full) or capture off (slim),
# so Prysm closes its capture and releases /tmp/prysm-capture.lock. The full
# worker and the supervisor may keep running; the probe takes the same lock and
# refuses (exit 1) while anything holds it. This script never stops or starts
# Prysm. Everything lives in a private /tmp/prysm-capability.* directory on
# the TV. The only file outside it is one Luna manifest, shaped like setup's
# worker manifest, giving the probe's own path the worker's capture role. It is
# removed afterwards only if it is still exactly ours, then Luna is rescanned.
# The probe runs under `timeout -s TERM 30` with its own 5 s per-call watchdog.
set -uo pipefail
tv=${PRYSM_TV:?set PRYSM_TV to the TV ssh destination, e.g. root@<tv-address>}
probe=${1:?probe binary} out=${2:?local evidence directory}
shift 2
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
app=io.prysm.tv.capprobe
manifest=/var/luna-service2/manifests.d/$app.json
version=$(sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+).*/\1/' "$root/VERSION")
mkdir -p "$out"
ssh_tv() { ssh -o BatchMode=yes -o ConnectTimeout=8 "$tv" "$@"; }
# This TV's luna-send exits without a reply when its stdin reaches EOF.
luna() { (sleep 13) | ssh_tv "timeout 12 luna-send -n 1 -w 10000 $*"; }

# Processes whose executable path starts with $1 (exact path or a prefix).
pids() {
  ssh_tv "for p in /proc/[0-9]*/exe; do e=\$(readlink \"\$p\") || continue; case \$e in $1*) p=\${p#/proc/}; echo \${p%/exe};; esac; done"
}
if [ -n "$(pids /tmp/prysm-capability.)" ]; then
  echo 'an earlier probe still runs on the TV; leaving it alone' >&2
  exit 1
fi
if ssh_tv "test -e $manifest"; then
  echo "$manifest already exists; remove it by hand after checking it is a probe leftover" >&2
  exit 1
fi
dir=$(ssh_tv 'mktemp -d /tmp/prysm-capability.XXXXXX') || exit 1
case $dir in /tmp/prysm-capability.??????*) ;; *) echo "unexpected directory $dir" >&2; exit 1;; esac
exe=$dir/vt_capability_probe

role=$(python3 -I - "$root/packaging/webos/service/worker.role.json" "$exe" <<'EOF'
import json, sys
role = json.load(open(sys.argv[1]))
role['exeName'] = sys.argv[2]
print(json.dumps(role))
EOF
) || exit 1
manifest_json="{\"id\":\"$app\",\"version\":\"$version\",\"roleFiles\":[\"$dir/probe.role.json\"]}"

cleanup() {
  echo '== cleanup'
  # TERM, never KILL: a probe killed inside a native call can hold the capture.
  left=$(pids "$exe")
  if [ -n "$left" ]; then
    # The probe finishes its current call, then releases: up to 4 calls of 5 s.
    ssh_tv "kill -TERM $left; timeout 25 sh -c 'for p in $left; do while kill -0 \$p 2>/dev/null; do sleep 0.2; done; done'"
    left=$(pids "$exe")
  fi
  if [ -n "$left" ]; then
    echo "probe $left still runs after TERM; leaving $dir and $manifest for inspection" >&2
    return
  fi
  if ssh_tv "cmp -s $manifest $dir/manifest.json"; then
    ssh_tv "rm $manifest && timeout 20 ls-control scan-services"
    echo "== removed $manifest, rescanned"
  elif ssh_tv "test -e $manifest"; then
    echo "$manifest changed; leaving it and $dir for inspection" >&2
    return
  fi
  ssh_tv "rm -rf -- $dir" && echo "== removed $dir"
}
trap cleanup EXIT

write() { # remote path, mode; content on stdin. noclobber: never overwrite.
  ssh_tv "set -euC; umask 077; cat > $1; chmod $2 $1"
}
write "$exe" 700 < "$probe" || exit 1
printf '%s\n' "$role" | write "$dir/probe.role.json" 644 || exit 1
printf '%s\n' "$manifest_json" | write "$dir/manifest.json" 644 || exit 1
printf '%s\n' "$manifest_json" | write "$manifest" 644 || exit 1
ssh_tv 'timeout 20 ls-control scan-services' || exit 1

luna "luna://com.webos.service.videooutput/getStatus '{}'" > "$out/video-before.json"
ssh_tv 'uname -a; tr "\0" " " < /proc/device-tree/compatible; echo' > "$out/platform.txt" 2>&1
echo "== probe $*"
ssh_tv "cd $dir && timeout -s TERM 30 ./vt_capability_probe $*" > "$out/probe.txt" 2> "$out/probe.stderr"
echo "exit=$?" >> "$out/probe.stderr"
luna "luna://com.webos.service.videooutput/getStatus '{}'" > "$out/video-after.json"
ssh_tv 'dmesg | tail -n 80' > "$out/dmesg-tail.txt" 2>&1
grep '^#' "$out/probe.txt"; cat "$out/probe.stderr"
