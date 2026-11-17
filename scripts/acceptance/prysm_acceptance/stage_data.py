"""What the `tv` and `eyes` stages' steps pass on to later ones (`plan.Context.data`).

A key is filled by the step that provides it (a setup step, mostly); a later
step that reads it before then fails with a KeyError, which the run reports
as that step's failure.
"""

from typing import TypedDict

from prysm_acceptance.commands import Flavour
from prysm_acceptance.prysm_api import Prysm, Settings, Target
from prysm_acceptance.tv import Preferences
from prysm_acceptance.web import Json


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
