"""The bench's controlled feed: a Chromecast with Google TV on the capture card's HDMI input.

`ChromecastCast` is the cast feed (`feed_cast.Cast`) with what the
Chromecast needs first: over ADB it keeps the Chromecast awake (put back at
the end) and reads its Wi-Fi address, which the clip is then cast to.
"""

import re
import shutil
import time
from typing import ClassVar

from prysm_acceptance import commands, feed_cast
from prysm_acceptance.checks import Bench, Failed
from prysm_acceptance.commands import Command
from prysm_acceptance.config import ChromecastFeed

ADB_HINT = (
    'ADB does not reach the Chromecast {serial}: plug it into this computer by USB with USB debugging '
    'on (Settings > System > Developer options) and allow this computer on the TV; for wireless '
    'debugging, run adb connect HOST:PORT yourself and give HOST:PORT as feed.chromecast.adb_serial'
)


class ChromecastCast(feed_cast.Cast):
    """The Chromecast `[PLATFORM.feed.chromecast]` names: `show` plays the clip and registers its stops."""

    RECEIVER = 'the Chromecast'
    CLEANUPS: ClassVar[tuple[str, ...]] = (
        *feed_cast.stopped(RECEIVER),
        "the Chromecast's stay_on_while_plugged_in put back to the value read before "
        "(adb -s SERIAL shell 'settings put global stay_on_while_plugged_in VALUE')",
    )

    def __init__(self, settings: ChromecastFeed) -> None:
        super().__init__(settings, 'CHROMECAST')  # its address is read over ADB when it is shown
        self.serial = settings.adb_serial

    def preflight(self) -> None:
        if not shutil.which(commands.adb_get_state(self.serial).argv[0]):
            raise Failed('the Chromecast feed needs adb (Android platform tools) on PATH')
        super().preflight()

    def actions(self) -> list[str]:
        return [
            f'ADB {self.serial}: get-state is device; stay_on_while_plugged_in read, set to {commands.STAY_ON} '
            '(put back at the end); wake; mWakefulness=Awake; the wlan0 address read',
            *super().actions(),
        ]

    def commands(self) -> list[Command]:
        serial = self.serial
        return [
            commands.adb_get_state(serial),
            commands.adb_stay_on_get(serial),
            commands.adb_stay_on_put(serial, commands.STAY_ON),
            commands.adb_wake(serial),
            commands.adb_wakefulness(serial),
            commands.adb_wifi_address(serial),
        ]

    def reach(self, ctx: Bench, evidence: dict[str, object]) -> str:
        """Keep the Chromecast awake (ADB) and read its Wi-Fi address."""
        evidence['keep_awake'] = keep_awake(ctx, self.serial)
        return wifi_address(ctx, self.serial)


def keep_awake(ctx: Bench, serial: str) -> dict[str, object]:
    """Check ADB, keep the screen on while powered (put back at the end) and wake it."""
    state = ctx.run(commands.adb_get_state(serial), check=False)
    if state.stdout.strip() != 'device':
        raise Failed(ADB_HINT.format(serial=serial), (state.stdout + state.stderr).strip())
    read = ctx.run(commands.adb_stay_on_get(serial)).stdout.strip()
    # A setting never written reads as null; the default is 0.
    before = 0 if read == 'null' else int(read) if read.isdigit() else -1
    if not 0 <= before <= 15:
        raise Failed(f'stay_on_while_plugged_in reads {read!r}, not 0 to 15')

    def restore() -> str:
        ctx.run(commands.adb_stay_on_put(serial, before))
        return f'stay_on_while_plugged_in {before}'

    ctx.cleanup('feed.awake', 'Chromecast keep-awake put back', restore)
    ctx.run(commands.adb_stay_on_put(serial, commands.STAY_ON))
    ctx.run(commands.adb_wake(serial))
    power = ''
    for _ in range(10):
        power = ctx.run(commands.adb_wakefulness(serial), check=False).stdout.strip()
        if 'mWakefulness=Awake' in power:
            return {'stay_on_before': before, 'power': power}
        time.sleep(0.5)
    raise Failed('the Chromecast did not wake: mWakefulness is not Awake after 5 s', power)


def wifi_address(ctx: Bench, serial: str) -> str:
    """The Chromecast's IPv4 address on wlan0, read over ADB."""
    line = ctx.run(commands.adb_wifi_address(serial)).stdout
    found = re.search(r'\binet ((?:[0-9]{1,3}\.){3}[0-9]{1,3})/', line)
    if not found or not all(int(part) <= 255 for part in found[1].split('.')):
        raise Failed(
            'the Chromecast has no Wi-Fi address: connect it to the same network as this computer', line.strip()
        )
    return found[1]
