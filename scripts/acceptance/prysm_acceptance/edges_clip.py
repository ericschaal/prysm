"""The generated test clip and the colour check that reads it back.

The clip is gray with a band along each edge, a fifth of the picture deep.
Each band is split in the middle, and each corner's colour fills the two
half-bands that meet there. At the first step the corners are red
(top-left), green (top-right), blue (bottom-right) and yellow (bottom-left).
Every `STEP_SECONDS` the colours move one corner on, clockwise; four steps
make a period. Nothing moves within a step.

Read clockwise, each edge shows two colours and its second is the next
edge's first. So an edge read backwards shows a pair no step has, and the
check catches a wrong colour, a reversed (mirrored) edge, swapped edges, a
frozen picture and steps at the wrong pace. It does not measure the delay
from the screen to the LEDs.
"""

from collections.abc import Sequence
from enum import StrEnum
from itertools import pairwise
from typing import TypedDict

from prysm_acceptance import colour_page
from prysm_acceptance.plan import Failed
from prysm_acceptance.prysm_api import ColorsEvent, Edge

# The corners' colours at the first step, clockwise from the top-left corner.
COLOURS = ('red', 'green', 'blue', 'yellow')
STEPS = 4
STEP_SECONDS = 2
# 30 minutes, longer than the whole stage, so a feed that does not loop never
# reaches the end. A whole number of periods, so a file source that repeats
# keeps the order. About 8 s to encode and 2 MB.
CLIP = {'width': 640, 'height': 360, 'fps': 30, 'seconds': 1800}

# The colours stream reports `top` and `bottom` left to right, `left` and
# `right` top to bottom (`LedColors` in crates/protocol/src/colors.rs; WLED's
# own order, clockwise from the bottom-left corner, is applied only when
# sending). So each edge's entries run from the first corner to the second
# (corners numbered clockwise from the top-left, 0 to 3):
EDGE_CORNERS: dict[Edge, tuple[int, int]] = {'top': (0, 1), 'right': (1, 2), 'bottom': (3, 2), 'left': (0, 3)}

# The entries of an edge that are read: those wholly within 10-40 % and
# 60-90 % of its length. That keeps out the corners and the middle, where
# overscan, scaling, compression and the sampler's reach blur the halves.
# With 8 entries (the stream's usual): 1-2 and 5-6.
HALVES = ((0.1, 0.4), (0.6, 0.9))

# How long the check listens: five steps, so four or five changes.
CHECK_SECONDS = 10.0
# How far a change may be from STEP_SECONDS after the one before. The stream
# looks every 100 ms, so each change arrives up to 0.1 s late; a capture adds
# a frame or two.
PACE_TOLERANCE = 0.3
# The longest a sample that is no step may last (from its arrival to the next
# step's first sample) and still count as the change between two steps: two of
# the stream's 100 ms looks and a frame or two of a capture caught mid-change.
TRANSITION = 0.3


class Fault(StrEnum):
    """What is wrong with a sample that is no step, in the order the check reports them."""

    WRONG_COLOUR = 'wrong colour'
    SWAPPED_EDGES = 'swapped edges'
    REVERSED_EDGE = 'reversed edge'


class Sample(TypedDict):
    """One event of the stream as the check read it: its arrival, its step or fault, each edge's halves."""

    t: float
    step: int | None
    fault: str | None
    edges: dict[Edge, str | None] | None


class Stretch(TypedDict):
    """Samples that are no step, since the last step's: from when, the first fault, the step before."""

    start: float
    fault: str | None
    before: int | None


# [first arrival, step] of each run of samples showing one step.
Run = list[float]


def corner_hex() -> tuple[str, ...]:
    """`COLOURS` as hex, for the clip's template."""
    return tuple('{:02x}{:02x}{:02x}'.format(*colour_page.PALETTE[name]) for name in COLOURS)


def expected(step: int) -> dict[Edge, tuple[str, ...]]:
    """Each edge's two colours at `step`, in the stream's order."""
    return {
        edge: tuple(COLOURS[(corner - step) % STEPS] for corner in corners) for edge, corners in EDGE_CORNERS.items()
    }


def seen(event: ColorsEvent) -> dict[Edge, tuple[str, ...] | None]:
    """Each edge's two halves by nearest named colour ('mixed' when a half's entries differ).

    None for an edge with too few entries to read two halves."""
    found: dict[Edge, tuple[str, ...] | None] = {}
    for edge in EDGE_CORNERS:
        entries: list[list[int]] = event.get(edge) or []
        count = len(entries)
        parts = [
            [entries[i] for i in range(count) if low <= i / count and (i + 1) / count <= high] for low, high in HALVES
        ]
        if not all(parts):
            found[edge] = None
            continue
        names = [{colour_page.nearest(colour) for colour in part} for part in parts]
        found[edge] = tuple(name.pop() if len(name) == 1 else 'mixed' for name in names)
    return found


def decode(event: ColorsEvent) -> tuple[int | None, str | None]:
    """(step, None) when every edge shows its two colours of one step, else (None, fault).

    A fault is compared with the step most edges agree on: an edge showing
    that step's pair backwards is reversed; one showing another edge's pair
    is swapped; anything else is a wrong colour."""
    if event.get('type') != 'colors':
        return None, f'no colours: {event.get("type")}'
    found = seen(event)
    short = [edge for edge, pair in found.items() if pair is None]
    if short:
        return None, f'too few entries to read two halves on {", ".join(short)}'
    agree = [sum(found[edge] == pair for edge, pair in expected(step).items()) for step in range(STEPS)]
    step = agree.index(max(agree))
    if agree[step] == len(EDGE_CORNERS):
        return step, None
    wanted = expected(step)
    found_faults: dict[Fault, list[str]] = {}
    for edge, pair in found.items():
        shown = ', '.join(pair or ())
        if pair == wanted[edge]:
            continue
        if pair == wanted[edge][::-1]:
            found_faults.setdefault(Fault.REVERSED_EDGE, []).append(
                f'{edge} shows {shown}, step {step} has {", ".join(wanted[edge])}'
            )
            continue
        other = [name for name, colours in wanted.items() if pair in (colours, colours[::-1])]
        if other:
            found_faults.setdefault(Fault.SWAPPED_EDGES, []).append(f"{edge} shows {shown}, {other[0]}'s colours")
        else:
            found_faults.setdefault(Fault.WRONG_COLOUR, []).append(
                f'{edge} shows {shown}, step {step} has {", ".join(wanted[edge])}'
            )
    kind = next(fault for fault in Fault if fault in found_faults)
    return None, f'{kind}: ' + '; '.join(text for fault in Fault for text in found_faults.get(fault, []))


def judge(samples: Sequence[tuple[float, ColorsEvent]], seconds: float) -> tuple[str, dict[str, object]]:
    """Judge the timed colours stream: `samples` are (seconds, event) by arrival over `seconds`.

    The stream sends only on change, so each sample holds until the next one.
    Every sample must be a step of the clip, or no step for at most
    `TRANSITION` between one step and the next; at least two steps must be
    seen, in rotation order, changing `STEP_SECONDS` ± `PACE_TOLERANCE`
    apart. Returns (detail, evidence); raises Failed with the evidence."""
    decoded = [_read(at, event) for at, event in samples]
    runs: list[Run] = []
    evidence: dict[str, object] = {'seconds': seconds, 'samples': decoded, 'steps': runs}
    if not decoded:
        raise Failed(f'no colours event within {seconds} s', evidence)
    _steps(decoded, seconds, runs, evidence)
    if not runs:
        raise Failed('no step of the clip seen', evidence)
    if len(runs) == 1:
        raise Failed(f'frozen picture: step {runs[0][1]} for the whole {seconds:g} s', evidence)
    for (at, before), (_, after) in pairwise(runs):
        if after != (before + 1) % STEPS:
            raise Failed(
                f'steps out of order: step {before} at {at} s, then step {after} (the clip goes 0, 1, 2, 3)', evidence
            )
    changes = [at for at, _ in runs[1:]]
    intervals = [round(b - a, 2) for a, b in pairwise(changes)]
    evidence['intervals'] = intervals
    _pace(intervals, {'before the first change': changes[0], 'after the last change': seconds - changes[-1]}, evidence)
    spread = f'{min(intervals)}-{max(intervals)} s apart' if intervals else f'at {changes[0]} s'
    return (
        f'{len(runs)} steps in order, changes {spread}; every edge shows its two colours in the right direction'
    ), evidence


def _read(at: float, event: ColorsEvent) -> Sample:
    """One sample: its step or fault and, for colours, each edge's halves."""
    step, fault = decode(event)
    edges: dict[Edge, str | None] | None = (
        {edge: ', '.join(pair) if pair else None for edge, pair in seen(event).items()}
        if event.get('type') == 'colors'
        else None
    )
    return {'t': at, 'step': step, 'fault': fault, 'edges': edges}


def _steps(decoded: Sequence[Sample], seconds: float, runs: list[Run], evidence: dict[str, object]) -> None:
    """Add to `runs` each run of samples showing one step; stretches of no step in between must be short changes."""
    stretch: Stretch | None = None  # the samples that are no step, since the last step's
    for sample in decoded:
        step = sample['step']
        if step is None:
            stretch = stretch or {
                'start': sample['t'],
                'fault': sample['fault'],
                'before': int(runs[-1][1]) if runs else None,
            }
            continue
        if stretch:
            _transition(stretch, sample['t'], step, evidence)
            stretch = None
        if not runs or runs[-1][1] != step:
            runs.append([sample['t'], step])
    if stretch:
        _transition(stretch, seconds, None, evidence)


def _pace(intervals: Sequence[float], held: dict[str, float], evidence: dict[str, object]) -> None:
    """Changes `STEP_SECONDS` apart, and no step held longer than that at the start or the end."""
    low, high = STEP_SECONDS - PACE_TOLERANCE, STEP_SECONDS + PACE_TOLERANCE
    if any(not low <= interval <= high for interval in intervals):
        raise Failed(
            f'wrong pace: steps changed {intervals} s apart, the clip every {STEP_SECONDS} s (±{PACE_TOLERANCE} s)',
            evidence,
        )
    for what, value in held.items():
        if value > high:
            raise Failed(
                f'wrong pace: no change for {value:.2f} s {what}, the clip changes every '
                f'{STEP_SECONDS} s (±{PACE_TOLERANCE} s)',
                evidence,
            )


def _transition(stretch: Stretch, end: float, after: int | None, evidence: dict[str, object]) -> None:
    """Allow samples that are no step only briefly, between a step and the next.

    At the start or the end of the window the step on the other side is
    unknown, so a short stretch there counts as a change too."""
    held = round(end - stretch['start'], 2)
    before = stretch['before']
    at_change = before is None or after is None or after == (before + 1) % STEPS
    if held <= TRANSITION and at_change:
        return
    if before is not None and after == (before + 1) % STEPS:
        raise Failed(
            f'transition too long: from step {before} to step {after} took {held} s '
            f'(at most {TRANSITION} s); in between {stretch["fault"]}',
            evidence,
        )
    raise Failed(f'{stretch["fault"]} (for {held} s from {stretch["start"]} s)', evidence)
