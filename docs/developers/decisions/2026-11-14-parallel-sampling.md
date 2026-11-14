# Data-parallel sampling inside the processor

Date: 2026-11-14

The spatial sampler's three per-frame loops (cell averages, the lights'
weighted means and colour focus) run in chunks on a small rayon pool that
the `PrysmProcessor` owns: [`workers.rs`](../../../crates/processor/src/workers.rs).
This is a contained exception to
[one runtime, plain blocking adapters](2026-11-12-one-runtime-plain-adapters.md):
the processor stays one blocking call on the session thread. Every chunk has
finished before the call returns, chunks share no mutable state, and the
colours are the same, bit for bit, on any number of threads (tested).

- The pool is dedicated, never rayon's global one: `PrysmProcessor::new`
  uses half the cores, at most three, so the TV's interface and video
  keep the other half; `with_threads` sets the count. With one thread
  there is no pool and the loops run as plain loops.
- The threads are named `prysm-sampler-N`. Dropping the processor tells
  them to end; rayon does not join them, but no work is outstanding by then.
- Pool threads spin briefly before sleeping after each frame, which costs
  some CPU besides the work itself.

Why: on the TV (4 Cortex-A9 cores) the processor ran on one thread at
11-12 ms per 640x360 frame, plus 8-10 ms with colour focus, and these loops
are most of that cost. Per-stage times come from
`cargo run --release -p prysm-processor --example bench -- --threads N`.

Measured on the TV the same day (bench at low priority while the TV played
video and Prysm ran; NV12 640x360, ms per frame):

| Threads | No colour focus | Colour focus 10 % |
| --- | --- | --- |
| 1 | 11.0 | 19.6 |
| 2 | 7.2 | 15.3 |
| 3 | 6.4 | 14.0 |

Cell averages (7.0 ms) and the lights' means (3.2 ms) are 93 % of a frame
without colour focus and scale with threads. Colour focus barely does
(8.2 to 7.6 ms): most of it is preparing the frame's colour tables, which
stays on one thread. Two threads give most of the gain, hence half the
cores.

Not yet verified: whether the TV's menus stay smooth with the pool, and the
extra CPU the pool's threads spend spinning between frames. If it does not
pay, `with_threads(1)` from `new` returns to the single-threaded processor
without other changes.
