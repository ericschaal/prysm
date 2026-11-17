"""The fields the runner reads and the requests it sends, against Prysm's generated contracts.

The runner types only the fields its checks read (`prysm_api`, `tv`). When
Prysm renames or removes one of them, or a value of a closed set, this fails
here instead of on the bench. It reads the committed, generated examples and
schemas in frontend/packages/contracts/generated/ (`npm --prefix frontend run
codegen`); it never copies them.
"""

from collections.abc import Callable, Iterator
from enum import StrEnum
import json
from pathlib import Path
from typing import Any, cast
import unittest
from unittest import mock

from acceptance_fakes import untyped

from prysm_acceptance import config, edges_clip, host, host_cells, prysm_api, stage_eyes, tv, tv_cells
from prysm_acceptance.checks import Failed
from prysm_acceptance.options import Cell, Delivery, Flavour, Platform, Source
from prysm_acceptance.prysm_api import SessionState, Target
from prysm_acceptance.transport import Result
from prysm_acceptance.tv import Encoding, Power, Video
from prysm_acceptance.web import Reply

GENERATED = Path(__file__).resolve().parents[3] / 'frontend/packages/contracts/generated'
EXAMPLE = Path(__file__).resolve().parents[1] / 'config.example.toml'


def load(name: str) -> Any:
    return json.loads((GENERATED / name).read_text())


FIXTURES = load('fixtures.json')
APPLICATION = load('application.openapi.json')['components']['schemas']
LUNA = load('luna.openapi.json')['components']['schemas']
DESKTOP = load('host-desktop.openapi.json')['components']['schemas']


def names(schemas: Any, schema: Any) -> Iterator[str]:
    """Every property name a schema allows, through `$ref`, `allOf` and `oneOf`."""
    if '$ref' in schema:
        yield from names(schemas, schemas[schema['$ref'].rsplit('/', 1)[1]])
    yield from schema.get('properties', {})
    for part in (*schema.get('allOf', ()), *schema.get('oneOf', ())):
        yield from names(schemas, part)


def values(schemas: Any, name: str) -> set[str]:
    """The values of a closed set (a string enum, or a oneOf of them)."""
    schema = schemas[name]
    return {value for part in schema.get('oneOf', [schema]) for value in part.get('enum', ())}


def tags(schemas: Any, name: str, field: str) -> set[str]:
    """The values of a tagged union's tag `field` (each `oneOf` part names its own)."""
    return {value for part in schemas[name]['oneOf'] for value in part['properties'][field]['enum']}


def read(kind: Any) -> set[str]:
    """The fields a TypedDict of the runner declares, that is the fields its checks read."""
    return set(kind.__required_keys__ | kind.__optional_keys__)


class Fields(unittest.TestCase):
    """Every field the runner reads is one Prysm's contract has."""

    def test_the_portal_api(self) -> None:
        for kind, schema in (
            (prysm_api.State, 'State'),
            (prysm_api.Input, 'Input'),
            (prysm_api.Session, 'Session'),
            (prysm_api.Failure, 'Failure'),
            (prysm_api.OutputBrightness, 'DeviceBrightness'),
            (prysm_api.ModesAnswer, 'Modes'),
            (prysm_api.CaptureMode, 'Mode'),
            (prysm_api.Span, 'Span'),
            (prysm_api.Candidate, 'Candidate'),
            (prysm_api.ColorsEvent, 'ColorsEvent'),
        ):
            with self.subTest(schema):
                self.assertLessEqual(read(kind), set(names(APPLICATION, APPLICATION[schema])))

    def test_the_tv_service(self) -> None:
        reply = set(names(LUNA, LUNA['StateReply'])) | set(names(LUNA, LUNA['ErrorReply']))
        self.assertLessEqual(read(tv.LunaReply), reply)
        self.assertLessEqual(read(tv.Preferences), set(names(LUNA, LUNA['Preferences'])))
        self.assertLessEqual(read(tv.TvFailure), set(names(LUNA, LUNA['WorkerFailure'])))

    def test_the_closed_sets(self) -> None:
        sets: tuple[tuple[type[StrEnum], Any, str], ...] = (
            (Target, APPLICATION, 'LightingTarget'),
            (Target, LUNA, 'LightingTarget'),
            (Flavour, LUNA, 'Flavor'),
            (Encoding, LUNA, 'SlimEncoding'),
            (Power, LUNA, 'Power'),
            (Video, LUNA, 'Video'),
        )
        for kind, schemas, name in sets:
            with self.subTest(name):
                self.assertEqual({member.value for member in kind}, values(schemas, name))
        self.assertEqual({member.value for member in SessionState}, tags(APPLICATION, 'Session', 'state'))

    def test_the_calibration_patterns_eyes_shows(self) -> None:
        self.assertLessEqual(set(stage_eyes.PATTERNS), values(APPLICATION, 'Pattern'))


class Examples(unittest.TestCase):
    """The generated examples, read by the runner's own readers."""

    def test_states(self) -> None:
        for state in FIXTURES['State']:
            self.assertIn(state['target'], tuple(Target))
            self.assertIsInstance(state['revision'], int)
            self.assertIsInstance(state['settings'], dict)
            self.assertIsInstance(prysm_api.session(state), str)
        self.assertTrue(any(prysm_api.running(state) for state in FIXTURES['State']))
        rates = prysm_api.rates([state.get('input') for state in FIXTURES['State']])
        self.assertGreater(rates['fps_mean'], 0)
        self.assertGreater(rates['sent_mean'], 0)

    def test_modes_and_colours(self) -> None:
        for answer in FIXTURES['Modes']:
            if answer['status'] == 'known':
                self.assertEqual(len(prysm_api.describe_modes(answer['modes'])), len(answer['modes']))
        for event in FIXTURES['ColorsEvent']:
            self.assertIsNotNone(edges_clip.decode(event))
        for candidate in FIXTURES['Candidate']:
            self.assertIsInstance(candidate['label'], str)

    def test_luna_replies(self) -> None:
        for state in FIXTURES['luna/StateReply']:
            reply = tv.luna_reply(Result(0, 'garbage\n' + json.dumps(state) + '\n'))
            self.assertIn(reply['flavor'], tuple(Flavour))
            self.assertIsInstance(reply['worker']['phase'], str)
            self.assertIn(reply.get('lighting'), (None, *Target))


def sent(call: Callable[[prysm_api.Prysm], object]) -> Any:
    """The body of the last request `call` makes of a Prysm in the state of an example (answered 204)."""
    with (
        mock.patch.object(prysm_api.web, 'request', return_value=Reply(204, {}, b'')) as request,
        mock.patch.object(prysm_api.web, 'get_json', return_value=FIXTURES['State'][1]),
    ):
        call(prysm_api.Prysm('http://prysm.example'))
    return json.loads(json.dumps(cast('mock.MagicMock', request).call_args.args[2]))


class Requests(unittest.TestCase):
    """The bodies the runner sends are the contract's requests."""

    def test_target(self) -> None:
        self.assertIn(sent(lambda prysm: prysm.set_target(Target.PROCESSED)), FIXTURES['TargetRequest'])

    def test_settings(self) -> None:
        state = FIXTURES['State'][1]
        body = sent(lambda prysm: prysm.replace_settings(state['settings']))
        self.assertIn(body, FIXTURES['SettingsRequest'])


class WrittenSettings(unittest.TestCase):
    """The settings the runner writes use only fields a desktop Prysm has."""

    def fields(self, schema: str) -> set[str]:
        return set(names(DESKTOP, DESKTOP[schema]))

    def test_the_local_prysm_headless(self) -> None:
        document = untyped(host.settings(host.wled_output(config.load(EXAMPLE)), {'kind': 'remote'}))
        self.assertLessEqual(set(document), self.fields('Settings'))
        self.assertLessEqual(set(document['output']), self.fields('WledSettings'))
        processing = document['processing']
        self.assertLessEqual(set(processing), self.fields('Processing'))
        self.assertLessEqual(set(processing['custom_style']), self.fields('StyleValues'))
        self.assertLessEqual(set(processing['style']), self.fields('ChosenStyle'))

    def test_the_captures(self) -> None:
        bench = config.load(EXAMPLE)
        clip = 'edges.mp4'
        file = untyped(host_cells.capture_settings(Cell(Platform.MAC, Delivery.BINARY, Source.FILE), bench, clip))
        self.assertEqual((file['kind'], file['repeat']), ('file', True))
        self.assertLessEqual(set(file), self.fields('FileSettings'))
        device = untyped(host_cells.capture_settings(Cell(Platform.MAC, Delivery.BINARY, Source.DEVICE), bench, None))
        self.assertEqual((device['kind'], device['path'], device['fps']), ('device', bench.mac.device, 30))
        self.assertLessEqual(set(device), self.fields('DeviceSettings'))
        remote = untyped(tv_cells.capture(Flavour.SLIM, 640, 360, 30, encoding=Encoding.RAW))
        self.assertEqual(remote['encoding'], 'raw')
        self.assertLessEqual(set(remote), self.fields('RemoteReceiverSettings'))
        webos = untyped(tv_cells.capture(Flavour.FULL, 640, 360, 30))
        self.assertEqual(set(webos), set(FIXTURES['WebosSettings'][0]))
        self.assertEqual({member.value for member in Encoding}, values(DESKTOP, 'StreamEncoding'))


class Stub(prysm_api.Prysm):
    """A Prysm with a capture card in its settings, answering from the generated examples."""

    def __init__(self, modes: list[prysm_api.CaptureMode]) -> None:
        super().__init__('http://prysm.example')
        self.answer = {'status': 'known', 'modes': modes}

    def candidates(self, section: str, kind: str) -> list[prysm_api.Candidate]:
        return [c for c in FIXTURES['Candidate'] if 'path' in c['fields']]

    def state(self) -> prysm_api.State:
        capture = {'kind': 'device', 'path': '/dev/video0', 'width': 640, 'height': 360, 'fps': 30}
        return cast('prysm_api.State', {'settings': {'capture': capture}})

    def modes(self, capture: dict[str, object]) -> Reply:
        return Reply(200, {}, json.dumps(self.answer).encode())


def mode(width: int, height: int, fps: int) -> prysm_api.CaptureMode:
    return {'width': span(width), 'height': span(height), 'fps': span(fps)}


def span(value: int) -> prysm_api.Span:
    return {'min': value, 'max': value, 'step': 1}


class CaptureDevice(unittest.TestCase):
    """The configured device among the candidates, offering the size Prysm runs with; no name matching."""

    def test_an_offered_mode_passes(self) -> None:
        found = untyped(host_cells.device_offered(Stub([mode(1280, 720, 30), mode(640, 360, 30)]), '/dev/video0'))
        self.assertEqual(found['size'], {'width': 640, 'height': 360, 'fps': 30})

    def test_a_missing_id_lists_the_devices_present(self) -> None:
        with self.assertRaisesRegex(Failed, r"no capture device 'USB'; present: USB Video \(/dev/video0\)"):
            host_cells.device_offered(Stub([mode(640, 360, 30)]), 'USB')

    def test_a_size_no_mode_takes_fails(self) -> None:
        with self.assertRaisesRegex(Failed, r'offers no mode of 640x360 @ 30: 1280 x 720 @ 30'):
            host_cells.device_offered(Stub([mode(1280, 720, 30), mode(640, 360, 60)]), '/dev/video0')


if __name__ == '__main__':
    unittest.main()
