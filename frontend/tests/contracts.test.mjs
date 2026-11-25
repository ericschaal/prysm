import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import Ajv2020 from 'ajv/dist/2020.js';
import {
  renderConstants,
  renderDocuments,
  updateGeneratedFiles,
} from '../scripts/codegen.mjs';

const generated = new URL('../packages/contracts/generated/', import.meta.url);
const modules = JSON.parse(
  await readFile(new URL('manifest.json', generated), 'utf8'),
);
const documents = Object.fromEntries(
  await Promise.all(
    modules.map(async (name) => [
      name,
      JSON.parse(
        await readFile(new URL(`${name}.openapi.json`, generated), 'utf8'),
      ),
    ]),
  ),
);
const fixtures = JSON.parse(
  await readFile(new URL('fixtures.json', generated), 'utf8'),
);
// OpenAPI metadata/format annotations are not JSON Schema validation keywords.
const ajv = new Ajv2020({ strict: false, validateFormats: false });
// `module/Name` names a schema of one document; a plain `Name` is the first
// document that has it (the application roots are repeated in host documents).
const validate = (key) => {
  const [module, name] = key.includes('/') ? key.split('/') : [null, key];
  const document = module
    ? documents[module]
    : Object.values(documents).find((doc) => doc.components?.schemas?.[name]);
  assert(document?.components?.schemas?.[name], `Missing schema: ${key}`);
  return ajv.compile({ ...document, $ref: `#/components/schemas/${name}` });
};

test('generated schemas accept actual Rust serialization of every wire root', () => {
  assert(Object.keys(fixtures).length >= 5);
  for (const [name, examples] of Object.entries(fixtures)) {
    const check = validate(name);
    for (const example of examples)
      assert(check(example), `${name}: ${JSON.stringify(check.errors)}`);
  }
});

test('schemas reject unknown targets, untagged sections and incomplete failures', () => {
  const state = fixtures.State[1];
  assert.equal(validate('State')({ ...state, target: 'on' }), false);
  assert.equal(validate('State')({ ...state, revision: '7' }), false);
  assert.equal(
    validate('State')({ ...state, session: { state: 'faulted' } }),
    false,
  );
  assert.equal(
    validate('Settings')({
      ...state.settings,
      capture: { device: '/dev/video0' },
    }),
    false,
  );
  assert.equal(
    validate('Settings')({ ...state.settings, calibrations: [] }),
    false,
  );
  assert.equal(
    validate('Settings')({
      ...state.settings,
      processing: { ...state.settings.processing, brightness_percent: 101 },
    }),
    false,
  );
  assert.equal(validate('Failure')({ kind: 'invalid' }), false);
  // Each kind carries what it needs: a stale revision the current one.
  assert.equal(
    validate('Failure')({ kind: 'stale_revision', message: 'x' }),
    false,
  );
  assert(
    validate('Failure')({ kind: 'stale_revision', message: 'x', current: 2 }),
  );
  assert.equal(
    validate('LightingFailure')({ kind: 'busy', message: 'x' }),
    false,
    'the state carries only lighting failures',
  );
  assert.equal(validate('TargetRequest')({ target: 'off', id: 1 }), false);
});

test('webOS capture settings keep the ranges the adapter validates', () => {
  const check = validate('host-webos/WebosSettings');
  const [section] = fixtures.WebosSettings;
  assert(check({ kind: 'webos' }));
  for (const bad of [
    { fps: 61 },
    { fps: 0 },
    { width: 641 },
    { width: 1922 },
    { height: 1082 },
    { include_ui: 'yes' },
    { device: '/dev/video0' },
  ])
    assert.equal(check({ ...section, ...bad }), false, JSON.stringify(bad));
  assert.equal(
    validate('host-webos/CaptureSettings')({
      kind: 'device',
      device: '/dev/video0',
    }),
    false,
  );
});

test('the Luna document covers both flavors with one State', () => {
  const states = fixtures['luna/State'];
  // Each package in each worker phase: full asking with every fix,
  // running also with lights that don't answer and with what it asks,
  // stopping also to ask; slim running in each link state but starting and
  // sending.
  assert.deepEqual(
    states.map((state) => `${state.flavor} ${state.worker.phase}`).sort(),
    [
      ...Array(5).fill('full asking'),
      ...Array(3).fill('full running'),
      'full stopped',
      'full stopping',
      'full stopping',
      'slim asking',
      ...Array(7).fill('slim running'),
      'slim stopped',
      'slim stopping',
    ],
  );
  assert.deepEqual(
    [
      ...new Set(
        states.flatMap(({ worker }) =>
          worker.problem
            ? [worker.problem.asked.fix ?? worker.problem.asked.step]
            : [],
        ),
      ),
    ].sort(),
    [
      'change_settings',
      'restart_tv',
      'set_up',
      'try_again',
      'unanswered',
      'unsupported',
    ],
    'every kind of what is asked',
  );
  const [full] = states;
  const slim = states.find((state) => state.flavor === 'slim');
  const state = validate('luna/State');
  assert.equal(state({ ...full, flavor: 'other' }), false);
  assert.equal(state({ ...full, power: 'on' }), false);
  assert.equal(
    state({ ...full, unreadable: null }),
    false,
    'omitted, not null',
  );
  // A package's own fields are required, and the other flavor's do not fit.
  const { target, ...withoutTarget } = full;
  assert.equal(target, 'screen_sync');
  assert.equal(state(withoutTarget), false);
  assert.equal(state({ ...full, flavor: 'slim' }), false);
  assert.equal(state({ ...slim, flavor: 'full' }), false);
  assert.equal(
    state({ ...full, worker: { phase: 'running', capturing: 'yes' } }),
    false,
  );
  assert.equal(state({ ...full, worker: { phase: 'asking' } }), false);
  assert.equal(
    state({
      ...full,
      worker: {
        phase: 'asking',
        problem: { asked: { step: 'ask' }, message: 'x' },
      },
    }),
    false,
    '`ask` says its fix',
  );
  const preferences = validate('luna/Preferences');
  assert(preferences({}));
  assert.equal(preferences({ mode: 'slim' }), false);
  assert.equal(
    preferences({ capture: { ...states[1].preferences.capture, fps: 61 } }),
    false,
  );
  const reply = validate('luna/StateReply');
  assert.equal(reply({ ...states[0], returnValue: false }), false);
  const error = validate('luna/ErrorReply');
  assert(
    error({ returnValue: false, errorKind: 'forbidden', errorText: 'denied' }),
  );
  assert.equal(error({ returnValue: false, errorText: 'denied' }), false);
  assert.equal(validate('luna/SetEnabledRequest')({}), false);
  // Requests are closed: a target this version does not know is refused.
  assert(validate('luna/SetTargetRequest')({ target: 'effects' }));
  assert.equal(validate('luna/SetTargetRequest')({ target: 'ambient' }), false);
  assert.equal(validate('luna/UninstallRequest')({ force: true }), false);
});

test('drift checks fail without writing; generation repairs missing, changed and obsolete artifacts', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'prysm-codegen-'));
  const files = { 'types.ts': 'export type Rate = number;\n' };
  try {
    await assert.rejects(
      updateGeneratedFiles(directory, files, true),
      /Generated contracts differ/,
    );
    await updateGeneratedFiles(directory, files, false);
    await updateGeneratedFiles(directory, files, true);
    await writeFile(join(directory, 'types.ts'), 'drift');
    await writeFile(join(directory, 'obsolete.ts'), 'obsolete');
    await assert.rejects(
      updateGeneratedFiles(directory, files, true),
      /Generated contracts differ/,
    );
    assert.equal(await readFile(join(directory, 'types.ts'), 'utf8'), 'drift');
    await updateGeneratedFiles(directory, files, false);
    await updateGeneratedFiles(directory, files, true);
    await assert.rejects(readFile(join(directory, 'obsolete.ts')), {
      code: 'ENOENT',
    });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('an unfamiliar library and multiple public types need no generator mappings', async () => {
  const document = {
    openapi: '3.1.0',
    info: { title: 'Extension', version: '1' },
    paths: {},
    components: {
      schemas: {
        WidgetSettings: {
          type: 'object',
          properties: { enabled: { type: 'boolean' } },
          required: ['enabled'],
        },
        WidgetStatus: { type: 'string', enum: ['ready', 'waiting'] },
      },
    },
  };
  const files = await renderDocuments({ extension: document });
  assert(
    files['extension.generated.ts'].includes('export type WidgetSettings ='),
  );
  assert(
    files['extension.generated.ts'].includes('export type WidgetStatus ='),
  );
  assert.deepEqual(JSON.parse(files['manifest.json']), ['extension']);
  assert.deepEqual(JSON.parse(files['extension.openapi.json']), document);
  await assert.rejects(
    renderDocuments({ '../escape': document }),
    /Invalid contract module name/,
  );
});

test('constants are generated as literal values, with safe names only', () => {
  const file = renderConstants({ zeta: [1, 'two'], alphaBeta: { a: 0.5 } });
  assert(file.startsWith('/** Generated by cargo xtask codegen from Rust.'));
  assert(file.indexOf('export const alphaBeta = {') < file.indexOf('zeta'));
  assert(file.includes('"a": 0.5\n} as const;'));
  for (const name of ['Zeta', 'two-words', '__proto__', 'a b'])
    assert.throws(
      () => renderConstants({ [name]: 1 }),
      /Invalid constant name/,
      name,
    );
});
