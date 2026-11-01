# webOS capture lifecycle research — 2026-11-01

Investigation on one webOS 25 / 10.3.2-34 TV: native constructor leaks, shutdown hangs, and whether application-side fixes can avoid a separate capture process.

Start with the [complete diagnosis](cleanup-diagnosis.md), then the [Ghidra/Frida follow-up](report/2026-11-01_reverse-vt-lifecycle-report.md) for evidence and call paths.

The HAL handle-growth issue has a library-lifetime fix. Video construction can leak GLib resources and leave a freed object in a global registry. Native teardown and restart share a lost-wakeup race. Persistent driver reuse and one public lifecycle retry worked in short tests, but process-level recovery remains necessary after constructor failure or a shutdown deadline. These findings apply to the sampled firmware; they do not establish portability across TVs.

## Contents

| Location | Contents |
| --- | --- |
| [cleanup-diagnosis.md](cleanup-diagnosis.md) | Full investigation, existing code fixes, measurements and architecture implications |
| [report/](report/2026-11-01_reverse-vt-lifecycle-report.md) | Follow-up findings and evidence-to-call-path mapping |
| `scripts/` | Original C/C++ probes, wait-tracing shim, Python TV runner, Ghidra exporter and Gum build command |
| `evidence/` | Follow-up logs, tool metadata and twelve evidence records |
| `evidence/earlier/` | Earlier HAL, video, signal, permission-preflight and teardown logs |
| [scope.md](scope.md), [timeline.md](timeline.md), [workitems.md](workitems.md) | Authorization context and investigation history |
| [tool-index.md](tool-index.md), [field-journal.md](field-journal.md) | Tool versions and reusable research notes |

The Cargo diagnostic remains at [cleanup_probe.rs](../../../crates/capture/webos-capture/examples/cleanup_probe.rs), where it can use the production modules. Existing production fixes remain in their crates; this folder is a research archive, not a runtime dependency.

## Evidence preservation

Runtime logs and probe source snapshots were copied byte-for-byte from the temporary investigation. The two identical tracing/runner copies were consolidated as `scripts/wait-trace.c` and `scripts/run-tv-probe.py`; the earlier constructor harness is `scripts/constructor_probe.cpp`. Reports were adjusted for this location. Original temporary files remain as a backup.

Vendor binaries, disassembly, decompiler output, Ghidra project databases, downloaded SDK archives and compiled probes are excluded. [external-artifacts.json](evidence/external-artifacts.json) preserves the original paths and hashes of the external decompiler output. E-002 through E-004 explicitly mark this offline dependency rather than pointing to missing repository files. The complete diagnosis records the vendor binary hashes.

`evidence/artifact-sha256.txt` covers archived sources and evidence, excluding itself. Verify it from this folder:

```sh
shasum -a 256 -c evidence/artifact-sha256.txt
```

The original `.jsonl` files mix JSON snapshots with native text logging; they are raw console captures, not strict JSONL streams. Logs contain process IDs, addresses, library/device paths and resource metadata, but no captured pixel contents.

## Reproduction

Scripts are historical research snapshots. The TV runner retains the original `root@192.168.18.10` target and `/tmp` paths; inspect and adapt these before use on another setup. It writes a temporary executable and, with `--role`, a temporary Luna role/manifest, then removes them. It acquires the shared capture lock and runs an eight-second external watchdog. A watchdog timeout is not a successful native teardown. The tracing shim's offset is specific to the hashed vendor library.

Run from the repository root. With the original Mac webOS SDK installed, build the reuse probe and tracing shim into temporary storage:

```sh
research_dir=research/webos-capture/2026-11-01-lifecycle
capture_sdk=/Users/eschaal/webos-sdk/arm-webos-linux-gnueabi_sdk-buildroot
mkdir -p /tmp/prysm-reverse /tmp/prysm-cleanup-evidence
"$capture_sdk/bin/arm-webos-linux-gnueabi-g++" \
  --sysroot="$capture_sdk/arm-webos-linux-gnueabi/sysroot" \
  -Icrates/capture/webos-capture/native \
  -std=c++11 -O1 -mfloat-abi=softfp -mfpu=neon \
  "$research_dir/scripts/reuse_probe.cpp" \
  -o /tmp/prysm-reverse/reuse_probe -ldl -pthread
"$capture_sdk/bin/arm-webos-linux-gnueabi-gcc" \
  --sysroot="$capture_sdk/arm-webos-linux-gnueabi/sysroot" \
  -shared -fPIC -O1 -mfloat-abi=softfp -mfpu=neon \
  "$research_dir/scripts/wait-trace.c" \
  -o /tmp/prysm-vt-trace.so -ldl -pthread
```

For an authorized, available TV with the same firmware and existing active input, the eight-cycle control is:

```sh
python3 "$research_dir/scripts/run-tv-probe.py" \
  --role --binary /tmp/prysm-reverse/reuse_probe \
  --name archived-reuse-control -- 8
```

New logs go to `/tmp/prysm-cleanup-evidence`, preserving the archived evidence. The retry variant builds the same way with `reuse_retry_probe.cpp`; its delayed-wait run uses `--trace --delay 2` and three cycles. Gum requires the compatible openlgtv SDK libraries described in the report. `scripts/build-gum-command.txt` preserves the exact original command; adjust its source path to this archive. Ghidra reproduction also needs the authorized vendor ELF copies, kept outside this repository.

This move did not rerun hardware tests, alter the installed service, or commit changes. Evidence checks do not revalidate the measured hardware behavior.
