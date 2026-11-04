# Native webOS service

The Rust Luna supervisor owns TV settings, permission setup, power observation
and one capture worker. The TV app uses Luna subscriptions; no Node runtime is
needed. See [lifecycle and permissions](../../docs/developers/webos.md) for
production behavior and [packaging](../../docs/developers/packaging.md#webos) for
building both binaries.

## Build and FFI generation

Checked-in bindings come from the target SDK's Luna/GLib headers. Regenerate with
the same SDK, libclang, rustfmt and workspace bindgen version:

```sh
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  cargo run --locked -p prysm-webos-service --features codegen --bin generate-luna-bindings
```

Add `-- --check` to verify without writing. Codegen uses ARMv7/Cortex-A9,
soft-float/NEON and the SDK sysroot, with host include discovery disabled. Both
headers must parse before output is written. Bindgen is absent from production.

Generated function tables retain loaded libraries. `src/luna.rs` owns handwritten
Luna FFI; `src/worker/native.rs` owns the fork/exec/descriptor boundary. Host unit
tests need no SDK or TV libraries. Target loading uses
`/usr/lib/libluna-service2.so.3` and `/usr/lib/libglib-2.0.so.0`; another ABI needs
separate validation.

## Process adapter and UI contract

Service configuration and worker startup share validated `InstanceUrl`, `Source`
and `Compression` types from `prysm-webos-protocol`. Verify a changed receiver URL
with a bounded probe before saving. Pass typed JSON through `PRYSM_WORKER_CONFIG`,
never a shell. The worker requires event and readiness pipes.

`config.rs` owns values, `files.rs` owns persistence/registrations and `App`
handles typed requests. Authorize Luna callers before parsing settings. The
protocol crate owns events/control bytes; native handles stay inside FFI.
`std::process::Child` owns the worker; `nix` provides pipes, descriptor flags and
signals. `pre_exec` clears CLOEXEC only on the inherited writer and installs Linux
parent-death SIGKILL with a PPID recheck. Reap before restarting.

`cargo xtask codegen webos` exports actual serialized settings/status types into
TV schemas, types and validators. Add `--check` to verify. See
[frontend generation](../../docs/developers/frontend.md#api-generation).

## Unsafe boundary contract

`luna::run` accepts Rust values and returns a Rust result. Callers have no
memory-safety preconditions: they cannot supply native pointers/callbacks, move
native state across threads or free native resources. The wrapper enforces:

| Concern | Required invariant |
| --- | --- |
| Callback storage | One stable shared `Rc`; initialization/cleanup never take an exclusive reference. Native state cannot cross threads. |
| Dispatch | One private GLib context driven by the run thread; no nested/default-context dispatch. |
| Messages | Callback-local wrappers; only owned JSON reaches application logic. Each deferred reply owns exactly one explicit ref, released on reply/error or before unregister. |
| Errors | Initialize before reading; copy text before freeing, including during Rust unwind. |
| Teardown | Mark closing, remove timer, release deferred messages, cancel power observation, unregister Luna, then release callback storage, loop/context and libraries. Partial startup follows the same rules. |
| Failure | Callback panic aborts without unwinding into C or dropping the panic payload. Failed unregister aborts rather than freeing retained storage. |
| ABI | SDK-generated layouts/functions; loaded libraries must honor ABI, strings and ownership contracts. |

Application logic denies unsafe code. Generated/handwritten boundaries deny
implicit unsafe operations, and handwritten blocks require safety comments.
Host stubs exercise reentrant callbacks, partial initialization, cleanup order,
malformed payloads and fatal paths. They do not establish LG library safety.

## Miri

CI runs host and ARMv7 soft-float boundary tests under pinned Miri:

```sh
rustup toolchain install nightly-2026-11-01 --profile minimal --component miri,rust-src --no-self-update
cargo +nightly-2026-11-01 miri test --locked -p prysm-webos-service --target armv7-unknown-linux-gnueabi
```

Miri uses Rust-owned library handles and ABI stubs, without native `dlopen`.
Keep aliasing/leak checks enabled. Filesystem/process and subprocess-abort tests
run in the ordinary harness. Miri checks wrapper ownership, not vendor libraries.

## Isolated hardware test

```sh
python3 packaging/webos/tests/native-smoke.py --host root@TV_IP
```

Requires root SSH keys. Default mode is read-only and needs no capture worker.
`--supervisor` also tests setup against sandbox paths/fake scanner and exercises a
worker; stop competing capture first. It uses a private `/tmp` directory, separate
`io.prysm.capture.probe.service` identity and exclusive volatile manifest at
`/var/luna-service2/manifests.d/io.prysm.capture.probe.json`.

The test checks Luna activation, power observation, requests/subscriptions,
malformed input, SIGTERM, reactivation and expiry. Keep its `finally` cleanup and
30-second process expiry. After interrupted SSH, remove only the owned probe
manifest and rescan before deleting its referenced files. Never replace unrelated
manifest content. The probe does not change installed Prysm settings/boot hooks,
TV power, kernel/rootfs or other system partitions.

Without arguments the service uses production paths as root with no expiry.
Never use that as a probe. `--run-for 1..300` is read-only;
`--sandbox /tmp/prysm-supervisor.XXXXXX --run-for 30` permits mutations only in an
existing private 0700 directory. Its workers/scanners stay inside that directory.

## Verification

```sh
cargo test --locked -p prysm-webos-service --all-features
```

Run target hardware checks separately after changing native contracts.
