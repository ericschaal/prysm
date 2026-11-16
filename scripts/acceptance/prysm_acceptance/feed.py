"""The feed: what puts a picture into the capture card.

A feed the runner controls shows the generated test clip on the card's
input, so the colour checks can compare each edge with the clip. Its
`show(ctx, clip)` plays the clip and registers, as cleanups, what stops it
again; `settled()` waits until the picture can be trusted and checks that
the clip still plays. It also describes itself for the dry run (`how`,
`actions`, `commands`, `CLEANUPS`) and checks its tools (`preflight`).

The bench describes its feed in the configuration (`[feed.chromecast]`); a
Chromecast with Google TV is the one supported. Without a feed the card
records whatever is plugged into it.
"""
import feed_chromecast


def configured(config):
    """The bench's controlled feed, or None."""
    return feed_chromecast.Feed(config.feed) if config.feed else None
