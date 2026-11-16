"""The fields the runner reads and the requests it sends, against Prysm's generated contracts.

The runner types only the fields its checks read (`prysm_api`, `tv`). When
Prysm renames or removes one of them, or a value of a closed set, this fails
here instead of on the bench. It reads the committed, generated examples and
schemas in frontend/packages/contracts/generated/ (`npm --prefix frontend run
codegen`); it never copies them.
"""

from collections.abc import Callable, Iterator
import contextlib
from enum import StrEnum
import json
from pathlib import Path
from typing import Any, cast
import unittest
from unittest import mock

from acceptance_fakes import untyped

from prysm_acceptance import config, edges_clip, host, modes, plan, prysm_api, stage_desktop, transport, tv, tv_slim
from prysm_acceptance.commands import Flavour
from prysm_acceptance.options import Source
from prysm_acceptance.plan import Context
from prysm_acceptance.prysm_api import Target
from prysm_acceptance.stage_data import DesktopData
from prysm_acceptance.transport import Result
from prysm_acceptance.tv_slim import Encoding
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
        )
        for kind, schemas, name in sets:
            with self.subTest(name):
                self.assertEqual({member.value for member in kind}, values(schemas, name))


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
                self.assertEqual(len(modes.describe(answer['modes'])), len(answer['modes']))
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
        file = sent(self.file_source)['settings']['capture']
        self.assertEqual((file['kind'], file['repeat']), ('file', True))
        self.assertLessEqual(set(file), self.fields('FileSettings'))
        remote = sent(lambda prysm: tv_slim.request(prysm, 640, 360, 30, Encoding.RAW))
        self.assertEqual(remote['settings']['capture']['encoding'], 'raw')
        self.assertLessEqual(set(remote['settings']['capture']), self.fields('RemoteReceiverSettings'))
        self.assertEqual({member.value for member in Encoding}, values(DESKTOP, 'StreamEncoding'))

    def file_source(self, prysm: prysm_api.Prysm) -> None:
        """The desktop stage's video source step with the generated clip."""
        ctx = Context[DesktopData](config.parse({}), Options(), transport.Refusing(), Path())
        ctx.data.update(prysm=prysm, clip='edges.mp4')
        with contextlib.suppress(plan.Failed):  # the mocked reply is no 200
            stage_desktop.source(ctx)


class Options:
    source, source_path, no_build, display_check, sweep, tv_is_free = Source.FILE, None, True, False, False, False


if __name__ == '__main__':
    unittest.main()
