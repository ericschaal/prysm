"""What the runner's tests share: colours as a capture shows them, and typing helpers.

The host cells are tested against the real prysm-headless on loopback
(test_acceptance_host.py), not against a stand-in Prysm.
"""

from typing import Any, cast

from prysm_acceptance.checks import Bench
from prysm_acceptance.options import Options
from prysm_acceptance.prysm_api import ColorsEvent, Edge
from prysm_acceptance.transport import Transcript

# The checks under test read no option.
NO_OPTIONS = cast('Options', None)


def untyped(value: object) -> Any:
    """A value as a test reads it (evidence, mostly): of whatever shape the code under test gave."""
    return value


def transcript_of(ctx: Bench) -> Transcript:
    """The recorded transport a test's bench runs on."""
    return cast('Transcript', ctx.transport)


# Named colours as a capture shifts them a little.
RGB = {
    'red': [235, 60, 40],
    'green': [50, 230, 60],
    'blue': [45, 70, 235],
    'yellow': [240, 235, 45],
    'gray': [70, 70, 70],
}


def edges_event(pairs: dict[Edge, tuple[str, ...]]) -> ColorsEvent:
    """A ColorsEvent with 8 entries per edge: four of each of its two named colours, in the stream's order."""
    event: ColorsEvent = {'type': 'colors'}
    for edge, (first, second) in pairs.items():
        event[edge] = [RGB[first]] * 4 + [RGB[second]] * 4
    return event
