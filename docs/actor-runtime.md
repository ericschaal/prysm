# Actor runtime

`prysm` starts one Actix runtime for HTTP and control when using WLED output.
Axum sends typed `SetTarget` and `SetConfig` messages to the `Controller` actor.
It owns configuration, operation state, and periodic WLED observations. Active
operations reject competing mutations; shutdown waits for accepted work before
stopping the pipeline. Configuration writes are serialized with `AtomicResponse`.
`Pipeline` serializes
start, stop, and configuration commands. Each active session owns a dedicated
Arbiter for capture → processing → configured rendering.

Data travels through typed recipients. Frame and color mailboxes have capacity
one; `try_send` drops incoming data when full. Processing and rendering handlers
are synchronous. Lifecycle commands await replies and preserve ordering.

Prysm owns one TOML configuration and saves it atomically. Processing
changes send `ConfigureProcessor`; input, resolution, or output changes stop and
recreate the session. HTTP listen and allowed-host changes require an application
restart; the running listener retains its original access policy until then.

Each actor reports terminal results through a bounded completion channel. The
lifecycle actor exposes phase, processed-frame count, applied revision, and
failure details to HTTP. EOF completes the session; failures leave HTTP available.
Starting the application never turns on capture or restores saved power intent.

Stopping cancels capture, drains accepted processing and rendering, waits for
the camera worker to release its device, then joins the Arbiter. The WLED Off/effects operation follows that join, so no Prysm packet
can race the final WLED command. A five-second drain timeout stops the Arbiter
and reports an error. No child-process protocol or separate worker binary is used.

Run locally:

```sh
cargo run -p prysm -- --config prysm.toml
# In another terminal:
cd apps/prysm-web/control-ui
npm ci
npm run dev
```

Open http://localhost:5173. For production, run `cargo run -p prysm-web` and
open http://localhost:8788. The build installs npm dependencies and builds/embeds
the UI; Node.js 24 and npm are required. This independent process forwards REST calls to Prysm
on port 8787. It owns no pipeline or settings file; stopping it leaves capture
and configuration untouched.

## Native output

Select `[output] type = "egui"` and build Prysm with `--features desktop`.
`application::run` keeps egui on the OS main thread while an Actix runtime thread
serves REST and runs the configuration and pipeline lifecycle actors. Capture
starts idle for either output. Any REST client can start/stop it and apply settings.

`input::into_stream` selects file, local device, or webOS capture, independently
of output. Each renderer receives `RenderFrame` (source, crop, and edge colors);
the WLED actor uses colors, while the egui actor publishes frames and colors to
the window. At EOF the last egui frame remains visible and REST stays available;
stop/start replays the file. Window close and signals shut down Prysm, drain
accepted operations, and join the pipeline. See `prysm-egui.toml` for an example.

WLED HTTP management is optional. Without it, power represents pipeline activity
and no WLED HTTP requests occur. With it, the configuration actor coordinates
capture with WLED power/effects and observes physical lighting state.

Changing output kind saves the configuration, stops capture,
and requests a Prysm restart. Builds without `desktop` reject egui settings before
saving; startup and `check` enforce the same validation. Processing changes apply
live through messages. REST listen/access policy changes also require a restart.
