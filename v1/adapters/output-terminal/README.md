# Terminal output

`prysm-output-terminal` implements `OutputControl` and `ColorSink`. It displays
processed colors around the four screen edges using Ratatui and Crossterm, with
application/capture status, frame lineage, processing revisions and six recent
diagnostics. It replaces WLED in this composition; it does not observe or mirror
physical LED delivery.

Use [the terminal configuration](../../assemblies/examples/terminal.json), selecting
your capture device, then run from the repository root:

```sh
cargo run --manifest-path v1/Cargo.toml -p prysm-assemblies \
  --features headless --bin prysm-headless -- /path/to/terminal.json --start
```

Without `--start` the host stays idle. Stdout must be an interactive terminal.
Ctrl-C and SIGTERM use the normal host shutdown path. There is no raw mode or
keyboard-input worker. The dashboard separates current host status from the last
application snapshot, labels diagnostic severity and shows newest events first.
Below 72 columns or 22 rows it switches to a compact layout, prioritizing host
status and the latest diagnostic; very small terminals omit the LED preview.
Diagnostics retain six bounded messages, with as many visible as space allows.
Resizing is handled by Ratatui. Large LED counts are sampled into the available cells, preserving the
screen orientation and converting linear colors with the existing sRGB conversion.

## Ownership and transitions

Construction is inert. The returned `owner` owns one joined terminal thread;
`control` and `colors` go to the application and flow. Optional `diagnostics`
accepts presentation updates only. The host retains `shutdown`, which immediately
and permanently closes admission. No public handle writes to the terminal.

| Input | Behavior |
| --- | --- |
| Configure | Validate LED counts and require no unfinished session; draw configured output |
| Hold | Fence the epoch, discard obsolete candidates and draw the supplied colors |
| Admit | Admit exactly the requested epoch; draw its newest candidate or held colors |
| Offer | Nonblocking; retain one newest eligible candidate |
| Finish Off | Seal the session, draw black and clear frame metadata before acknowledging |
| Native | Unsupported; capabilities report `native_mode: false` |
| Diagnostics / resize | Redraw the current display; never recover colors from a sealed session |
| Shutdown | Fence all new work, finish any owned draw, reject queued commands, restore styling/cursor/screen and join |

The [shared admission rules](../../crates/output/src/lib.rs) are also used by WLED.
The [mailbox](src/mailbox.rs) owns inputs, notifications, worker registration and
atomic observations. The [worker](src/worker.rs) serializes all controls and draws;
successful control replies follow the draw/flush. Frames, diagnostics and resizing
redraw at most ten times per second; control transitions are immediate. The
[screen](src/screen.rs) owns terminal restoration; [UI](src/ui.rs) is pure layout. Diagnostics are
bounded and control characters are removed before rendering. The assembly forwards
application observation updates and translates host events into `HostDiagnostic`
facts. The terminal adapter owns their wording, severity, and panel placement;
no presentation types or terminal dependencies enter the application contracts.

Blocking terminal writes happen on the retained worker, not the host executor.
Keep the owner future alive through shutdown. If it is cancelled, its Drop fallback
requests shutdown and joins synchronously. Failed restoration is `RecoveryRequired`,
including restoration after a partially failed terminal acquisition. Tests use a
gated display and Ratatui's `TestBackend`; no capture hardware is needed.
