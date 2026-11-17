"""The feed: what puts a picture into the capture card.

A feed the runner controls shows the generated test clip on the card's
input, so the colour checks can compare each edge with the clip. Its
`show(ctx, clip)` plays the clip and registers, as cleanups, what stops it
again; `settled()` waits until the picture can be trusted and checks that
the clip still plays. It also describes itself for the dry run (`how`,
`actions`, `commands`, `CLEANUPS`) and checks its tools (`preflight`).

The bench describes each platform's feed in the configuration
(`[mac.feed.chromecast]`); a Chromecast with Google TV is the one supported.
Without a feed the card records whatever is plugged into it.
"""

from typing import ClassVar, Protocol

from prysm_acceptance import feed_chromecast
from prysm_acceptance.checks import Bench
from prysm_acceptance.commands import Command
from prysm_acceptance.config import Machine


class Feed(Protocol):
    """A feed the runner controls (see the module's description)."""

    CLEANUPS: ClassVar[tuple[str, ...]]

    def preflight(self) -> None: ...
    def how(self) -> str: ...
    def actions(self) -> list[str]: ...
    def commands(self) -> list[Command]: ...
    def show(self, ctx: Bench, clip: str) -> tuple[str, object]: ...
    def settled(self) -> None: ...


def configured(machine: Machine) -> Feed | None:
    """The platform's controlled feed, or None."""
    return feed_chromecast.Feed(machine.feed) if machine.feed else None
