#!/usr/bin/env bash
# TV acceptance for one Prysm package, run by hand against a rooted TV:
#
#   tv-acceptance.sh [--dry-run] --flavour full|slim [--prysm http://HOST:8787]
#                    [--build-dir DIR] root@TV path/to/io.prysm.tv-FLAVOUR_VERSION_all.ipk
#
# 1. native-smoke.py: basic, --supervisor, --capture (slim streams to --prysm,
#    a running Prysm with the remote source and lighting on). Each runs the
#    cargo binaries from --build-dir under the isolated probe identity.
# 2. install-ipk.sh: installs the package and runs its setup.
# 3. The checks only a person can make (standby, cold boot, the remote...):
#    asked one by one, answered pass, fail or skip with a note.
#
# Everything is written to research/tv-acceptance/DATE-FLAVOUR.md (see the
# README there); a failed step stops the run and is recorded. --dry-run
# passes --dry-run to both scripts, prints the manual checks, asks nothing,
# writes no record and touches no TV. Builds and CI never call this.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)

dry_run=false
flavour=
prysm=
build_dir=
positional=()
while [ $# -gt 0 ]; do
    case $1 in
        --dry-run) dry_run=true ;;
        --flavour) flavour=${2:?--flavour takes full or slim}; shift ;;
        --prysm) prysm=${2:?--prysm takes a URL}; shift ;;
        --build-dir) build_dir=${2:?--build-dir takes a directory}; shift ;;
        -h | --help) sed -n '2,18p' "$0"; exit 0 ;;
        -*) echo "$0: unknown option $1" >&2; exit 2 ;;
        *) positional+=("$1") ;;
    esac
    shift
done
if [ ${#positional[@]} -ne 2 ] || [ -z "$flavour" ]; then
    echo "usage: $0 [--dry-run] --flavour full|slim [--prysm URL] [--build-dir DIR] root@TV IPK" >&2
    exit 2
fi
host=${positional[0]}
ipk=${positional[1]}
case $flavour in
    full) ;;
    slim) if [ -z "$prysm" ]; then echo "$0: slim needs --prysm http://HOST:8787" >&2; exit 2; fi ;;
    *) echo "$0: --flavour is full or slim" >&2; exit 2 ;;
esac
case $(basename "$ipk") in
    io.prysm.tv-"$flavour"_*_all.ipk) ;;
    *) echo "$0: $ipk is not a $flavour package (io.prysm.tv-${flavour}_VERSION_all.ipk)" >&2; exit 2 ;;
esac

# What only a person can check, per flavour.
manual=(
    "The TV app opens and shows Prysm's state in plain words"
    "Screen sync with video playing: the lights follow the picture"
    "Screen sync on the TV's menus without video"
    "Standby, then wake: lighting resumes as it was"
    "Cold boot with Start with TV on: lighting starts by itself"
    "A hung capture is killed and lighting recovers"
    "Calibration patterns show and clear"
    "Remote-control navigation reaches every control in the TV app"
    "Removal: the app and service are gone and the lights are off"
)
if [ "$flavour" = slim ]; then
    manual+=("Restart the Prysm at $prysm: the stream reconnects by itself")
fi

record=
if [ "$dry_run" = false ]; then
    mkdir -p "$ROOT/research/tv-acceptance"
    record="$ROOT/research/tv-acceptance/$(date +%Y-%m-%d)-$flavour.md"
    if [ -e "$record" ]; then
        record="${record%.md}-$(date +%H%M%S).md"
    fi
    {
        echo "# TV acceptance: $flavour, $(date +%Y-%m-%d)"
        echo
        echo "- Status: in progress"
        echo "- Package: $(basename "$ipk")"
        echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD)$(git -C "$ROOT" diff --quiet || echo ' (modified tree)')"
        echo "- TV: $host"
        [ "$flavour" = slim ] && echo "- Prysm: $prysm"
        echo "- Limits: one TV, one run; see the guide in [webos.md](../../docs/developers/webos.md)"
    } > "$record"
fi

note() {
    if [ -n "$record" ]; then
        printf '%s\n' "$@" >> "$record"
    fi
}

finish() {
    if [ -n "$record" ]; then
        sed -i.bak "s/^- Status: in progress$/- Status: $1/" "$record" && rm -f "$record.bak"
        echo "Recorded in ${record#"$ROOT"/}"
    fi
}

# Run one step, show and record its output; a failure ends the run.
step() {
    local title=$1
    shift
    echo "== $title"
    note "" "## $title" "" '```text' "\$ $*"
    local output status=0
    output=$(mktemp)
    "$@" 2>&1 | tee "$output" || status=$?
    if [ -n "$record" ]; then cat "$output" >> "$record"; fi
    rm -f "$output"
    note '```' "" "Exit status: $status"
    if [ "$status" -ne 0 ]; then
        echo "$0: $title failed ($status)" >&2
        finish "failed at $title"
        exit 1
    fi
}

smoke=(python3 "$HERE/native-smoke.py" --host "$host" --flavour "$flavour")
if [ -n "$build_dir" ]; then smoke+=(--build-dir "$build_dir"); fi
if [ "$dry_run" = true ]; then smoke+=(--dry-run); fi
capture=(--capture)
if [ "$flavour" = slim ]; then capture+=(--prysm "$prysm"); fi
install=("$HERE/../install-ipk.sh")
if [ "$dry_run" = true ]; then install+=(--dry-run); fi

step "Native smoke" "${smoke[@]}"
step "Native smoke: supervisor" "${smoke[@]}" --supervisor
step "Native smoke: capture" "${smoke[@]}" "${capture[@]}"
step "Install" "${install[@]}" "$host" "$ipk"

echo "== Manual checks"
note "" "## Manual checks" "" "| Check | Result | Note |" "| --- | --- | --- |"
failed=0
for check in "${manual[@]}"; do
    if [ "$dry_run" = true ]; then
        echo "- [ ] $check"
        continue
    fi
    answer=
    while :; do
        read -r -p "$check? [p]ass, [f]ail, [s]kip: " answer
        case $answer in
            p | pass) result=pass; break ;;
            f | fail) result=fail; failed=1; break ;;
            s | skip) result=skipped; break ;;
        esac
    done
    read -r -p "  note (optional): " text
    note "| $check | $result | ${text//|/\\|} |"
done

if [ "$dry_run" = true ]; then
    echo "Dry run: nothing was sent to a TV and nothing was recorded."
elif [ "$failed" -ne 0 ]; then
    finish "failed (manual checks)"
    exit 1
else
    finish "passed (skipped checks are listed)"
fi
