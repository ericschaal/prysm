#!/usr/bin/env bash
# Bounded TV session: disable Prysm, run the probe matrix, always re-enable.
# Usage: run-tv.sh <probe-binary> <local-evidence-dir> <matrix-file>
# Matrix lines: <name> <timeout-seconds> <probe arguments...>
set -uo pipefail
tv=${PRYSM_TV:-root@192.168.18.10}
dir=/media/developer/temp/ui-probe
probe=$1 out=$2 matrix=$3
mkdir -p "$out"
ssh_tv() { ssh -o BatchMode=yes "$tv" "$@"; }
luna() { (sleep 13) | ssh_tv "timeout 12 luna-send $*"; }
enable() { luna "-a io.prysm.tv -w 10000 -n 1 luna://io.prysm.tv.service/setEnabled '{\"enabled\":$1}'"; }
restore() {
  echo "== re-enable Prysm"; enable true
  ssh_tv "rm -rf $dir"; echo "== probe folder removed"
}
trap restore EXIT

ssh_tv "mkdir -p $dir" && scp -q -o BatchMode=yes "$probe" "$tv:$dir/probe" || exit 1
luna "-n 1 -w 5000 luna://com.webos.service.videooutput/getStatus '{}'" > "$out/video-before.json"
echo "== disable Prysm"; enable false
sleep 3
run() { # name, timeout seconds, args...
  local name=$1 t=$2; shift 2
  echo "== $name: $*"
  ssh_tv "cd $dir && timeout -s TERM $t ./probe $*" > "$out/$name.txt" 2> "$out/$name.stderr"
  echo "exit=$?" >> "$out/$name.stderr"
  grep '^#' "$out/$name.txt"; cat "$out/$name.stderr"
}
while read -r name t args; do
  case $name in ""|"#"*) continue;; esac
  run "$name" "$t" $args < /dev/null
done < "$matrix"
scp -q -o BatchMode=yes "$tv:$dir/frame-*.bgra" "$out/" 2>/dev/null
luna "-n 1 -w 5000 luna://com.webos.service.videooutput/getStatus '{}'" > "$out/video-after.json"
