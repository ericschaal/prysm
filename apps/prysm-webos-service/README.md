# Native webOS service

Luna supervisor: settings, permissions, power observation and one webOS capturer
worker. [Lifecycle](../../docs/developers/webos.md) · [Package build](../../docs/developers/packaging.md#webos)

## Build and FFI generation

With the target SDK, libclang and rustfmt:

```sh
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  cargo run --locked -p prysm-webos-service --features codegen --bin generate-luna-bindings
```

Add `-- --check` to verify. Bindings come from SDK headers, never host headers.
Host unit tests need no SDK. [luna.rs](src/luna.rs) owns Luna FFI;
[worker/native.rs](src/worker/native.rs) owns process/descriptor setup.

## Unsafe boundary contract

Safe callers supply only Rust values. The wrapper must preserve:

- Stable shared callback storage on one private GLib context/thread.
- Exactly one owned reference per deferred message; copy native errors before free.
- Teardown: mark closing → remove timer → release messages → cancel observation →
  unregister → free callback/context/library storage. Partial startup follows suit.
- Abort on callback panic or failed unregister; never unwind into C or free storage
  still retained by native code.
- SDK ABI, explicit descriptor ownership/CLOEXEC and Linux parent-death protection.

Authorize callers before parsing mutations. Pass typed worker config without a
shell; require event/readiness pipes and reap before restart.

## Miri

```sh
rustup toolchain install nightly-2026-11-01 --profile minimal --component miri,rust-src --no-self-update
cargo +nightly-2026-11-01 miri test --locked -p prysm-webos-service --target armv7-unknown-linux-gnueabi
```

Keep aliasing/leak checks. Stubs exercise wrapper ownership, not vendor libraries.

## Isolated hardware test

```sh
python3 packaging/webos/tests/native-smoke.py --host root@TV_IP
```

Requires root SSH keys. Default is read-only; `--supervisor` exercises a worker,
so stop competing capture. Keep the private `/tmp` directory, probe identity,
exclusive probe manifest, 30-second expiry and `finally` cleanup. After interrupted
SSH, remove only the owned probe manifest and rescan before deleting its files.
Never run unbounded production service mode as a probe.

## Verification

`cargo test --locked -p prysm-webos-service --all-features`
