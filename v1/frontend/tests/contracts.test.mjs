import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import Ajv2020 from 'ajv/dist/2020.js';
import { renderDocuments, updateGeneratedFiles } from '../scripts/codegen.mjs';

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
const validate = (name) => {
  const document = Object.values(documents).find(
    (doc) => doc.components?.schemas?.[name],
  );
  assert(document, `Missing schema: ${name}`);
  return ajv.compile({ ...document, $ref: `#/components/schemas/${name}` });
};

test('generated schemas accept actual Rust serialization for every adapter variant', () => {
  for (const [name, examples] of Object.entries(fixtures)) {
    const check = validate(name);
    for (const example of examples)
      assert(check(example), JSON.stringify(check.errors));
  }
});

test('schemas reject wrong variants, invalid resolution/rate and wrong LED tuple length', () => {
  const device = fixtures.DeviceCaptureSettings[0];
  const tv = fixtures.TvCaptureSettings[0];
  const output = fixtures.WledSettings[0];
  assert.equal(
    validate('DeviceCaptureSettings')({ ...device, source: 'remote' }),
    false,
  );
  assert.equal(
    validate('DeviceCaptureSettings')({
      ...device,
      path: undefined,
      peer: '127.0.0.1:9000',
    }),
    false,
  );
  assert.equal(
    validate('RemoteCaptureSettings')({ peer: '127.0.0.1:9000', rate: 30 }),
    false,
  );
  assert.equal(
    validate('ServerCaptureSettings')({ adapter: 'remote', settings: device }),
    false,
  );
  for (const resolution of [[0, 180], [3841, 180], [320, 2161], [320]]) {
    assert.equal(validate('TvCaptureSettings')({ ...tv, resolution }), false);
  }
  assert.equal(validate('TvCaptureSettings')({ ...tv, rate: 0 }), false);
  assert.equal(
    validate('TvCaptureSettings')({ ...tv, rate: 4294967296 }),
    false,
  );
  assert.equal(validate('WledSettings')({ ...output, leds: [1, 2, 3] }), false);
  assert.equal(
    validate('WledSettings')({ ...output, leds: [1, 2, 3, 4294967296] }),
    false,
  );
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
