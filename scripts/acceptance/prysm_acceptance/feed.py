"""The feed: what puts the generated test clip on the picture Prysm captures.

A feed the runner controls shows the clip, so the colour checks can compare
each edge with it. Its `show(ctx, clip)` plays the clip and registers, as
cleanups, what stops it again; `settled()` waits until the picture can be
trusted and checks that the clip still plays. It also describes itself for
the dry run (`how`, `actions`, `commands`, `CLEANUPS`) and checks its tools
(`preflight`).

The configuration names each feed. A host cell's capture card has its
platform's (`[mac.feed.chromecast]`: a Chromecast with Google TV on the
card's input, `feed_chromecast`); without one the card records whatever is
plugged into it. The TV cells always have the TV's (the clip cast to the TV's
own Cast receiver with a known overlay over it, `feed_cast.TvCast`;
`[tv.feed.cast]` only tunes it).
"""

from typing import ClassVar, Protocol

from prysm_acceptance import feed_cast, feed_chromecast
from prysm_acceptance.checks import Bench
from prysm_acceptance.commands import Command
from prysm_acceptance.config import Machine, Tv


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
    """The platform's controlled feed into its capture card, or None."""
    return feed_chromecast.ChromecastCast(machine.feed) if machine.feed else None


def on_tv(tv: Tv) -> feed_cast.TvCast:
    """The TV's controlled feed."""
    return feed_cast.TvCast(tv.feed, tv.host or 'TV')
