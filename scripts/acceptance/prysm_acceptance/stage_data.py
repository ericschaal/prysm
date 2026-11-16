"""What each stage's steps pass on to later ones (`plan.Context.data`).

A key is filled by the step that provides it (a setup step, mostly); a later
step that reads it before then fails with a KeyError, which the run reports
as that step's failure.
"""

from pathlib import Path
from typing import TypedDict

from prysm_acceptance.commands import Flavour
from prysm_acceptance.feed import Feed
from prysm_acceptance.host import Headless
from prysm_acceptance.prysm_api import Prysm, Settings, Target
from prysm_acceptance.tv import Preferences
from prysm_acceptance.web import Json, JsonObject
from prysm_acceptance.wled_api import Snapshot, Wled


class DesktopData(TypedDict, total=False):
    wled: Wled
    local_address: str
    wled_snapshot: Snapshot
    clip: str
    clip_edges: bool  # the clip is the generated one, so the colour check knows its edges
    feed: Feed
    settings_path: Path
    headless: Headless
    prysm: Prysm
    capture: JsonObject
    fps: int


class AsFound(TypedDict, total=False):
    """The TV as the `tv` stage found it, put back at the end."""

    flavor: Flavour | None
    preferences: Preferences | None
    lighting: Target | None
    enabled: bool | None
    settings: Settings | None


class TvData(TypedDict, total=False):
    as_found: AsFound
    ipk: dict[Flavour, str]
    full_settings: Settings
    receiver: Prysm
    receiver_for_tv: str


class EyesData(TypedDict, total=False):
    portal: Prysm | None
    dither: Json  # output.dither before the dithering checks
