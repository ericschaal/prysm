"""Stage C: what only a person can judge, one question at a time.

Where the portal can set up the state a check needs (lighting on, dithering
off, a calibration pattern), the runner does that first through the portal
API and puts it back afterwards.
"""

from collections.abc import Callable
from typing import TypedDict

from prysm_acceptance import commands, prysm_api, tv, web, webos
from prysm_acceptance.checks import Answer, Failed, Skipped
from prysm_acceptance.config import Config
from prysm_acceptance.plan import Context, Mode, Step
from prysm_acceptance.prysm_api import Prysm, Target, section
from prysm_acceptance.web import Json


class EyesData(TypedDict, total=False):
    """What the steps pass on (`plan.Context.data`): a key is filled by the step that provides it."""

    portal: Prysm | None
    dither: Json  # output.dither before the dithering checks


Ctx = Context[EyesData]
# Sets up, through the portal, the state a question needs; what it did.
Prepare = Callable[[Ctx, prysm_api.Prysm], str]

# The TV app's screens and dialogs (frontend/packages/tv-ui).
TV_SCREENS: list[tuple[str, str]] = [
    (
        'home',
        'Home: the title, lights card and settings read clearly from across the room. With lights off, '
        'Power is the only lighting action. With WLED on, Power, Screen sync and WLED effects are available.',
    ),
    ('portal-qr', 'Full: the portal address and QR code show; scanning it opens the portal on a phone'),
    ('waiting-video', 'With no video (Home screen of the TV or no input) the app says it is waiting for video'),
    ('navigation', 'Arrows reach every control in both columns; OK chooses; Back closes one level'),
]

# TV UI polish (prysm-jnu) a person still checks: the unit test that replaces it is not written yet (prysm-skn).
FLICKER: list[tuple[str, str]] = [
    ('no-rerender', 'Nothing flickers or re-renders visibly while idle (the QR code stays still)'),
]

PATTERNS = ['gray_192', 'red', 'green', 'blue', 'review_grid']
CLEANUPS = ('dithering set back to its value before the dithering checks', 'a calibration pattern cleared')


def _tv(ctx: Ctx) -> webos.WebosTv | None:
    """The configured TV, for its portal address and its state."""
    return webos.WebosTv(ctx.transport, ctx.config.tv.ssh) if ctx.config.tv.ssh else None


def _portal(ctx: Ctx) -> prysm_api.Prysm | None:
    url = ctx.config.eyes_portal
    found = _tv(ctx)
    if not url and found:
        url = tv.portal(found.get_state() or webos.LunaReply()) or ''
    if not url:
        return None
    try:
        prysm = prysm_api.Prysm(url)
        prysm.state()
        return prysm
    except (web.Unreachable, RuntimeError):
        return None


def question(text: str, prepare: Prepare | None = None) -> Callable[[Ctx], tuple[str, object]]:
    """A guided step: optional preparation through the portal, then one question."""

    def run(ctx: Ctx) -> tuple[str, object]:
        evidence: dict[str, object] = {}
        if prepare:
            prysm = ctx.data.get('portal', _portal(ctx))
            ctx.data['portal'] = prysm
            if prysm is None:
                evidence['prepared'] = 'no portal reachable: set the state by hand'
            else:
                evidence['prepared'] = prepare(ctx, prysm)
        answer, note = ctx.ask(text)
        evidence.update(answer=answer, note=note)
        return note or answer, evidence

    return run


def _lighting(ctx: Ctx, prysm: prysm_api.Prysm) -> str:
    reply = prysm.set_target(Target.SCREEN_SYNC)
    if reply.status != 204:
        raise Failed(f'could not turn screen sync on: HTTP {reply.status}', reply.json())
    return 'screen sync on'


def _dither(value: bool) -> Prepare:
    def prepare(ctx: Ctx, prysm: prysm_api.Prysm) -> str:
        _lighting(ctx, prysm)
        if 'dither' not in ctx.data:
            ctx.data['dither'] = section(prysm.state()['settings'], 'output').get('dither', True)
            ctx.cleanup('eyes.restore-dither', 'Dithering set back', lambda: _restore_dither(ctx, prysm))
        reply = prysm.change_settings(lambda s: section(s, 'output').__setitem__('dither', value))
        if reply.status != 200:
            raise Failed(f'dither {value}: HTTP {reply.status}', reply.json())
        return f'dither {"on" if value else "off"}'

    return prepare


def _restore_dither(ctx: Ctx, prysm: prysm_api.Prysm) -> str:
    original = ctx.data['dither']
    prysm.change_settings(lambda s: section(s, 'output').__setitem__('dither', original))
    return f'dither {original}'


def calibration_patterns(ctx: Ctx) -> tuple[str, object]:
    prysm = ctx.data.get('portal', _portal(ctx))
    if prysm is None:
        raise Skipped('no portal reachable for the patterns')
    _lighting(ctx, prysm)
    answers: dict[str, tuple[Answer, str]] = {}
    try:
        for pattern in PATTERNS:
            reply = prysm.calibration(pattern)
            if reply.status != 204:
                raise Failed(f'pattern {pattern}: HTTP {reply.status}', reply.json())
            answers[pattern] = ctx.ask(f'The TV shows the calibration pattern "{pattern}" and the lights match it?')
    finally:
        prysm.calibration(None)
    answers['cleared'] = ctx.ask('The pattern is gone and the lights follow the picture again?')
    failed = [name for name, (answer, _) in answers.items() if answer == Answer.NO]
    if failed:
        raise Failed(f'not as expected: {failed}', answers)
    if all(answer == Answer.SKIP for answer, _ in answers.values()):
        raise Skipped('all skipped', answers)
    return 'patterns shown and cleared', answers


def cold_boot(ctx: Ctx) -> tuple[str, object]:
    ready, note = ctx.ask(
        'On the full TV package, enable Start with TV. Start Screen sync, then turn the lights off with Prysm Power. '
        'In the WLED app, confirm the lights are off. Are these conditions ready?'
    )
    if ready != Answer.YES:
        raise Skipped('full package or physical Off precondition not confirmed', note)

    off, note = ctx.ask(
        'Unplug the TV for 30 s, plug it in and turn it on. Once the picture is back and Prysm has started, '
        'does it show Lights off with only Power available, while the physical lights stay off?'
    )
    evidence: dict[str, object] = {'off_after_boot': off, 'off_note': note}
    if off == Answer.SKIP:
        raise Skipped('cold boot skipped', evidence)
    if off == Answer.NO:
        raise Failed('fresh launch did not preserve physical Off', evidence)

    resumed, note = ctx.ask(
        'Press Prysm Power once without choosing a mode. Does the remembered Screen sync start and follow the picture?'
    )
    evidence.update(resumed=resumed, resume_note=note)
    if resumed == Answer.SKIP:
        raise Skipped('Power resume skipped', evidence)
    if resumed == Answer.NO:
        raise Failed('Power did not resume Screen sync', evidence)
    found = _tv(ctx)
    if found:
        state = tv.wait_state(found.get_state, tv.capturing, 120, every=5)
        evidence['getState'] = state
        if state is None:
            raise Failed('the TV did not confirm resumed capture', evidence)
    return 'Off reconciled and Power resumed Screen sync', evidence


def steps(config: Config) -> list[Step[EyesData]]:
    found: list[Step[EyesData]] = [
        Step(
            'eyes.lights-match',
            'Lights match the picture',
            'screen sync on through the portal; a person compares '
            'colour, position and timing on every edge with varied video',
            Mode.GUIDED,
            question(
                'Play varied video. Do the lights match the picture on every edge (colour, position, timing)?',
                _lighting,
            ),
        ),
        Step(
            'eyes.dark-scenes',
            'Dark scenes and WLED gamma',
            "a person watches dark scenes: hue kept, even fades (Prysm follows WLED's gamma)",
            Mode.GUIDED,
            question('Play a dark scene. Do dark colours keep their hue and fade evenly?', _lighting),
        ),
        Step(
            'eyes.dither-off',
            'Dithering off, dark room',
            'output.dither false through the portal; a person watches dark scenes in a dark room',
            Mode.GUIDED,
            question(
                'Dark room, dark scene, Smooth dark colours OFF: do the LEDs step visibly '
                '(answer y if this looks as expected for off)?',
                _dither(False),
            ),
        ),
        Step(
            'eyes.dither-on',
            'Dithering on, dark room',
            'output.dither true; the same scene; restored afterwards',
            Mode.GUIDED,
            question('Same scene with Smooth dark colours ON: smoother fades without twinkling?', _dither(True)),
        ),
    ]
    found += [
        Step(f'eyes.tv-{name}', f'TV app: {name}', text, Mode.GUIDED, question(text + '?')) for name, text in TV_SCREENS
    ]
    found += [
        Step(f'eyes.flicker-{name}', f'TV UI polish: {name}', text, Mode.GUIDED, question(text + '?'))
        for name, text in FLICKER
    ]
    found += [
        Step(
            'eyes.calibration-patterns',
            'Calibration patterns',
            f'PUT /api/v1/calibration {PATTERNS} one by one, '
            'a person confirms each on the TV and lights; DELETE clears it',
            Mode.GUIDED,
            calibration_patterns,
        ),
        Step(
            'eyes.calibration-wizard',
            'Calibration from the TV',
            'a person runs it',
            Mode.GUIDED,
            question(
                "From the TV app's Calibrate row: QR shown, the phone wizard runs end to end, the uncorrected "
                'toggle compares, Back ends it. All as expected?'
            ),
        ),
        Step(
            'eyes.portal-phone',
            'Portal on a phone',
            'a person uses the portal in a real phone browser',
            Mode.GUIDED,
            question(
                'Open the portal on your phone (Safari): Home, Settings (three groups), Calibrations, sliders '
                'and editors work by touch and read well. All good?'
            ),
        ),
        Step(
            'eyes.cold-boot',
            'Cold boot by unplugging',
            'full TV with WLED off: a person restarts the TV, verifies Off, presses Power, and confirms Screen sync',
            Mode.GUIDED,
            cold_boot,
            commands=lambda: [commands.tv_get_state(config.tv.ssh)] if config.tv.ssh else [],
        ),
    ]
    return found
