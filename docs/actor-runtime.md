# Actor runtime

`prysm` starts one Actix runtime for HTTP and control when using WLED output.
`application` composes the pipeline, lighting backend, controller, and HTTP server,
and owns their startup and shutdown. The control backend receives the existing
pipeline and status channel; constructing it does not start a pipeline or listener.
Axum sends typed `SetTarget` and `SetConfig` messages to the `Controller` actor.
It owns configuration, operation state, and periodic WLED observations. Active
operations reject competing mutations; shutdown waits for accepted work before
stopping the pipeline. Configuration writes are serialized with `AtomicResponse`.
Shutdown begins on the signal without waiting for HTTP requests to finish. After
the controller and pipeline stop, remaining HTTP requests get at most five seconds
to finish before the application exits.
`Pipeline` serializes
start, stop, and configuration commands. Each active session owns a dedicated
Arbiter for capture → processing → configured rendering.

Data travels through typed recipients. Frame and color mailboxes have capacity
one; `try_send` drops incoming data when full. Processing and rendering handlers
are synchronous. Lifecycle commands await replies and preserve ordering.
`prysm-core` owns frame data, pixel formats, color metadata, and pure pixel
conversion. Processing and desktop rendering depend on it directly, without
pulling in capture drivers or network receivers. `prysm-capture` re-exports those
types and conversions for existing callers.

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

Open http://localhost:5173 for frontend development. Prysm itself serves the
production UI and REST API at http://localhost:8787, using the same `[web]`
settings for both. Its build requires Node.js 24 and npm to embed the UI.
The optional standalone `prysm-web` process can host the UI separately and
forward API requests without owning a pipeline or settings file.

## Native output

Select `[output] type = "egui"` and build Prysm with `--features desktop`.
`application::run` keeps egui on the OS main thread while an Actix runtime thread
serves REST and runs the configuration and pipeline lifecycle actors. Capture
starts idle for either output. Any REST client can start/stop it and apply settings.

`InputSource::into_stream` selects file, local device, or webOS capture, independently
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

The lifecycle actor owns one `InputSource`. The input layer owns availability:
webOS keeps its listener open while configured, including while the pipeline is
idle; local device/file inputs retain configuration only. Pipeline startup asks
that source for a frame stream on the capture thread. The webOS receiver owns a
dedicated worker for accepting TVs, reading frames, decoding, and pixel conversion;
none of that frame work runs on the HTTP/control runtime. The worker stops when
its last receiver handle is dropped. Native capture resources
exist only for the active stream. Processing and rendering see the same frame
interface for every source. An idle TV receives `idle`, then capture settings on Start. Stop
closes the capture connection and waits for the receiver to release the stream;
the listening socket remains available for the TV's fresh worker. Input/address
changes replace the listener; bind failures leave the previous receiver in place
and report a failed configuration operation.
While idle, a replacement TV connection must complete a valid handshake before
it replaces the previous connection. A malformed client leaves the idle TV intact.

Starting screen sync reapplies the saved pipeline configuration before opening the
input. If a previous configuration attempt failed (for example, a listening port
was occupied), retry either applies those settings or leaves capture stopped.

The webOS frame stream has a separate availability signal. After disconnect or a
frame timeout, the pipeline reports `starting` with a waiting-for-frames message;
reconnection restores `running` after reception resumes. A queued frame cannot
clear the unavailable state. Stop still cancels the request and drains the session.

Shared `Frame` dimensions, format, and pixels are read-only after construction.
Consumers use `width()`, `height()`, `format()`, and `as_slice()`; YUV color metadata
belongs to the `PixelFormat::YUYV { range, matrix }` variant and is supplied by
the capture adapter at construction. RGB formats cannot carry YUV metadata.
