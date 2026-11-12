import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import {
  correctionLimits,
  hueLimits,
  processingDelayMs,
  processingLimits,
  profileLimits,
  staleNotice,
} from '@prysm/web-portal';
import { ledEdgeLimits, ledRuns, normalizeWledAddress, wledUrl } from '@prysm/output-wled-ui';
import { deviceDefaults, deviceLimits } from '@prysm/capture-device-ui';
import { fileDefaults, fileLimits } from '@prysm/capture-file-ui';
import { webosDefaults, webosLimits } from '@prysm/capture-webos-ui';
import { defaultProcessing } from '../testkit/application';
import {
  $,
  $$,
  button,
  click,
  edit,
  errorOf,
  field,
  form,
  renderPortal,
  sleep,
  submit,
  text,
} from './dom';

const capture = { kind: 'device', path: '/dev/video0' };
const output = { kind: 'wled', address: '192.0.2.1', leds: { top: 10, left: 4 } };
const mode = (target) => $(`[data-target="${target}"]`);
const hint = (element) =>
  document.getElementById(element.getAttribute('aria-describedby')).textContent;
const badge = (title) => $('.badge', form(title)).textContent;
const saves = (fixture) => fixture.calls.filter((call) => call === 'replaceSettings').length;
const press = (element) =>
  act(async () => {
    element.click();
    await sleep(0);
  });
const settle = () => act(async () => sleep(processingDelayMs + 100));
/** Another client replaces the settings. */
const elsewhere = (fixture, change) =>
  act(async () => {
    const { settings, revision } = fixture.snapshot();
    await fixture.client.replaceSettings(revision, { ...settings, ...change });
    await sleep(0);
  });

test('lighting modes set the target and show what the session is doing', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    assert.deepEqual(
      $$('[data-target]').map((item) => item.textContent),
      ['Screen sync', 'WLED effects', 'Off'],
    );
    assert.equal(mode('off').getAttribute('aria-pressed'), 'true');
    assert.equal($('.lighting-status').textContent, 'Lights are off.');

    await press(mode('processed'));
    assert.deepEqual(fixture.calls.slice(-1), ['target:processed']);
    assert.equal(mode('processed').getAttribute('aria-pressed'), 'true');
    assert.equal(mode('off').getAttribute('aria-pressed'), 'false');
    assert.equal($('.lighting-status').textContent, 'Screen sync is running.');

    await press(mode('device_native'));
    assert.deepEqual(fixture.calls.slice(-1), ['target:device_native']);
    assert.equal($('.lighting-status').textContent, 'WLED effects are on.');

    await press(mode('off'));
    assert.deepEqual(fixture.calls.slice(-1), ['target:off']);
    assert.equal($('.lighting-status').textContent, 'Lights are off.');

    const link = $('.device-link a');
    assert.equal(link.href, 'http://192.0.2.1/');
    assert.equal(link.target, '_blank');
    assert.match(link.rel, /\bnoopener\b/);
    assert.match(link.textContent, /^Open WLED/);

    await act(async () =>
      fixture.publish({
        target: 'processed',
        session: { state: 'stopped' },
        failure: { kind: 'adapter', message: 'WLED at 192.0.2.1 did not switch to live mode' },
      }),
    );
    assert.equal($('.lighting-status').textContent, 'Screen sync stopped.');
    assert.equal($('.lighting [role="alert"]').textContent, 'WLED at 192.0.2.1 did not switch to live mode');

    fixture.failNext({ kind: 'adapter', message: 'fake output cannot open' });
    await press(mode('processed'));
    assert(text().includes('fake output cannot open'));

    await act(async () =>
      fixture.publish({ session: { state: 'faulted', phase: 'closing_source' }, failure: null }),
    );
    assert.equal(
      $('.lighting-status').textContent,
      'Prysm stopped responding while closing the capture source. Restart Prysm to recover.',
    );
    assert($$('[data-target]').every((item) => item.disabled));
    assert.equal(hint(mode('off')), 'Restart Prysm to change the lights.');
  } finally {
    await portal.unmount();
  }
});

test('modes that cannot work are disabled and say why', async () => {
  for (const [initial, processed, native, effects] of [
    [{ output }, 'Configure capture first.', null, 'WLED effects'],
    [{ capture, output: { kind: 'null' } }, null, 'These lights have no effects of their own.', 'Device effects'],
    [{ capture }, 'Configure your lights first.', 'Configure your lights first.', 'Device effects'],
    [{}, 'Configure capture and lights first.', 'Configure your lights first.', 'Device effects'],
  ]) {
    const portal = await renderPortal(HeadlessWebPortal, initial);
    try {
      assert.equal(mode('processed').disabled, processed !== null);
      if (processed) assert.equal(hint(mode('processed')), processed);
      assert.equal(mode('device_native').disabled, native !== null);
      if (native) assert.equal(hint(mode('device_native')), native);
      assert.equal(mode('device_native').textContent, effects);
      assert.equal(mode('off').disabled, false);
      assert.equal(!!$('.device-link a'), initial.output?.kind === 'wled');
    } finally {
      await portal.unmount();
    }
  }
});

test('Open WLED derives http://host[:port]/ from the saved address', () => {
  for (const [address, url] of [
    ['192.168.1.50', 'http://192.168.1.50/'],
    ['wled.local', 'http://wled.local/'],
    ['WLED.local:8080', 'http://wled.local:8080/'],
    ['192.168.1.50:80', 'http://192.168.1.50/'],
    ['[fe80::1]:8080', 'http://[fe80::1]:8080/'],
    ['fe80::1', 'http://[fe80::1]/'],
    ['2001:db8::7', 'http://[2001:db8::7]/'],
    [' http://wled.local/ ', 'http://wled.local/'],
    ['', null],
    ['wled.local/path', null],
    ['user@wled.local', null],
    ['https://wled.local', null],
  ])
    assert.equal(wledUrl(address), url, address);
  assert.equal(normalizeWledAddress(' http://wled.local:8080/ '), 'wled.local:8080');
});

test('picture controls show the state, enforce ranges and apply live', async () => {
  const profiles = [{ id: 'ps5', name: 'PS5' }];
  const portal = await renderPortal(HeadlessWebPortal, { capture, output }, (snapshot) => ({
    settings: {
      ...snapshot.settings,
      processing: { ...snapshot.settings.processing, profiles, active_profile: 'ps5' },
    },
  }));
  const { fixture } = portal;
  const control = (name) => field(`processing.${name}`);
  const attributes = (element) => ['type', 'value', 'min', 'max', 'step'].map((name) => element[name] ?? '');
  try {
    assert.deepEqual(
      ['brightness_percent', 'shadow_lift_percent', 'spatial_reach_percent', 'smoothing_seconds', 'sample_density'].map(
        (name) => attributes(control(name)),
      ),
      [
        ['range', '100', '0', '100', '1'],
        ['range', '0', '0', '100', '1'],
        ['range', '10', '1', '100', '1'],
        ['number', '0', '0', '', 'any'],
        ['number', '1', '1', '65535', '1'],
      ],
    );
    assert.equal(control('remove_black_bars').checked, false);
    assert.equal(control('brightness_percent').labels[0].textContent, 'Brightness');

    // Two quick changes are applied once, after the user stops.
    await edit(control('brightness_percent'), 90);
    await edit(control('brightness_percent'), 80);
    assert.equal($('.processing output').textContent, '80%');
    assert.equal(saves(fixture), 0);
    await settle();
    assert.equal(saves(fixture), 1);
    assert.deepEqual(fixture.snapshot().settings, {
      capture,
      output,
      processing: { ...defaultProcessing, brightness_percent: 80, profiles, active_profile: 'ps5' },
    });

    await edit(control('sample_density'), 0);
    assert.equal(control('sample_density').getAttribute('aria-invalid'), 'true');
    assert.equal(errorOf(control('sample_density')), 'Enter a whole number from 1 to 65535.');
    await edit(control('smoothing_seconds'), -1);
    assert.equal(errorOf(control('smoothing_seconds')), 'Enter a number of at least 0.');
    await settle();
    assert.equal(saves(fixture), 1);

    await edit(control('sample_density'), 128);
    await edit(control('smoothing_seconds'), 0.5);
    await click(control('remove_black_bars'));
    assert.equal(errorOf(control('sample_density')), undefined);
    await settle();
    assert.equal(saves(fixture), 2);
    assert.deepEqual(fixture.snapshot().settings.processing, {
      ...defaultProcessing,
      brightness_percent: 80,
      sample_density: 128,
      smoothing_seconds: 0.5,
      remove_black_bars: true,
      profiles,
      active_profile: 'ps5',
    });

    // A field rejected by the server is marked and not retried until edited.
    fixture.failNext({ kind: 'invalid', field: 'processing.shadow_lift_percent', message: 'must be 0-100' });
    await edit(control('shadow_lift_percent'), 30);
    await settle();
    assert.equal(errorOf(control('shadow_lift_percent')), 'must be 0-100');
    await settle();
    assert.equal(saves(fixture), 3);
    await edit(control('shadow_lift_percent'), 20);
    await settle();
    assert.equal(saves(fixture), 4);
    assert.equal(fixture.snapshot().settings.processing.shadow_lift_percent, 20);
  } finally {
    await portal.unmount();
  }
});

test('the capture form saves the exact section on Save, with dirty state and reset', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture: { ...capture, fps: 25 }, output });
  const { fixture } = portal;
  try {
    assert.deepEqual(
      ['capture.path', 'capture.width', 'capture.height', 'capture.fps'].map((name) => field(name).value),
      ['/dev/video0', '640', '360', '25'],
    );
    assert.deepEqual(
      ['capture.width', 'capture.height', 'capture.fps'].map((name) => [field(name).min, field(name).max]),
      [['1', '3840'], ['1', '2160'], ['1', '2147483647']],
    );
    assert.equal(badge('Capture'), 'Saved');
    assert.equal(button('Save capture settings').disabled, true);

    await edit(field('capture.width'), 1280);
    assert.equal(badge('Capture'), 'Unsaved changes');
    await click(button('Reset', form('Capture')));
    assert.equal(field('capture.width').value, '640');
    assert.equal(badge('Capture'), 'Saved');

    await edit(field('capture.width'), 1280);
    await edit(field('capture.height'), 720);
    assert.equal(saves(fixture), 0);
    await submit(form('Capture'));
    assert.deepEqual(fixture.snapshot().settings, {
      capture: { kind: 'device', path: '/dev/video0', fps: 25, width: 1280, height: 720 },
      output,
      processing: defaultProcessing,
    });
    assert.equal(badge('Capture'), 'Saved');

    // Out of range: shown at once, never sent, focused on Save.
    await edit(field('capture.width'), 4000);
    assert.equal(errorOf(field('capture.width')), 'Enter a whole number from 1 to 3840.');
    await submit(form('Capture'));
    assert.equal(saves(fixture), 1);
    assert.equal($('[data-form-alert]', form('Capture')).textContent, 'Correct the highlighted fields before saving.');
    assert.equal(document.activeElement, field('capture.width'));

    // An empty path is reported once a save is tried.
    await click(button('Reset', form('Capture')));
    await edit(field('capture.path'), ' ');
    await submit(form('Capture'));
    assert.equal(errorOf(field('capture.path')), 'Enter a value.');
    assert.equal(saves(fixture), 1);
  } finally {
    await portal.unmount();
  }
});

test('server field errors land on their control and clear when it is edited', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    fixture.failNext({ kind: 'invalid', field: 'capture.height', message: 'height exceeds 2160' });
    await edit(field('capture.height'), 2000);
    await submit(form('Capture'));
    assert.equal(field('capture.height').getAttribute('aria-invalid'), 'true');
    assert.equal(errorOf(field('capture.height')), 'height exceeds 2160');
    assert.equal(document.activeElement, field('capture.height'));
    assert.equal($('[role="alert"]', form('Capture')).textContent, 'Not saved: height exceeds 2160');
    await edit(field('capture.height'), 1080);
    assert.equal(errorOf(field('capture.height')), undefined);
    assert.equal(field('capture.height').getAttribute('aria-invalid'), null);

    fixture.failNext({ kind: 'invalid', field: 'output.address', message: 'a unicast device address is required' });
    await edit(field('output.address'), '224.0.0.1');
    await submit(form('Lights'));
    assert.equal(errorOf(field('output.address')), 'a unicast device address is required');
    assert.equal(document.activeElement, field('output.address'));

    fixture.failNext({ kind: 'invalid', field: 'output.leds', message: 'total LED count must be between 1 and 65535' });
    await edit(field('output.address'), '192.0.2.7');
    await submit(form('Lights'));
    assert.equal($('[data-field-message="output.leds"]').textContent, 'total LED count must be between 1 and 65535');
    assert.equal(document.activeElement, $('[data-field="output.leds"]'));
    // Editing any edge answers the group's error.
    await edit(field('output.leds.right'), 2);
    assert.equal($('[data-field-message="output.leds"]'), null);

    // A field this form does not show still explains itself.
    fixture.failNext({ kind: 'invalid', field: 'output.unknown', message: 'unknown field' });
    await submit(form('Lights'));
    assert.equal(document.activeElement, $('[data-form-alert]', form('Lights')));
    assert.equal(document.activeElement.textContent, 'Not saved: unknown field');
    // Every attempt reached the server and was refused; nothing was saved.
    assert.equal(saves(fixture), 4);
    assert.equal(fixture.snapshot().revision, 0);
  } finally {
    await portal.unmount();
  }
});

test('a section changed elsewhere is reloaded instead of overwritten', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    // Seen before saving: the draft is kept until Save, which reloads.
    await edit(field('capture.path'), '/dev/video1');
    await elsewhere(fixture, { capture: { kind: 'device', path: '/dev/video9' } });
    assert(text().includes('Changed elsewhere since you started editing.'));
    assert.equal(field('capture.path').value, '/dev/video1');
    await submit(form('Capture'));
    assert.equal(saves(fixture), 1);
    assert.equal(field('capture.path').value, '/dev/video9');
    assert.equal($('.notice[role="status"]', form('Capture')).textContent, staleNotice);

    // Refused by the server (409): reloaded the same way.
    await edit(field('capture.path'), '/dev/video2');
    fixture.failNext({ kind: 'stale_revision', message: 'settings revision 1 is stale; current is 2', current: 2 });
    await submit(form('Capture'));
    assert.equal(field('capture.path').value, '/dev/video9');
    assert.equal($('.notice[role="status"]', form('Capture')).textContent, staleNotice);
    assert.equal(fixture.snapshot().settings.capture.path, '/dev/video9');

    // Clean forms follow other clients.
    await elsewhere(fixture, { output: { ...output, address: '192.0.2.50' } });
    assert.equal(field('output.address').value, '192.0.2.50');

    // Saving other sections (here a live picture change) is not a conflict.
    await edit(field('capture.path'), '/dev/video3');
    await edit(field('processing.brightness_percent'), 50);
    await settle();
    await submit(form('Capture'));
    assert.equal($('.notice[role="status"]', form('Capture')).textContent, 'Saved.');
    assert.equal(fixture.snapshot().settings.capture.path, '/dev/video3');
    assert.equal(fixture.snapshot().settings.processing.brightness_percent, 50);
    assert.equal(fixture.snapshot().settings.output.address, '192.0.2.50');
  } finally {
    await portal.unmount();
  }
});

test('kind selectors swap panels without losing drafts or the other section', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    assert.deepEqual($$('option', field('capture.kind')).map((item) => [item.value, item.textContent]), [
      ['device', 'Video capture device'],
      ['remote', 'Capture-only TV (remote)'],
      ['file', 'Video file'],
    ]);
    assert.deepEqual($$('option', field('output.kind')).map((item) => item.value), ['wled', 'null']);

    await edit(field('output.address'), 'wled.local');
    await edit(field('capture.kind'), 'remote');
    assert.equal(field('capture.path'), null);
    assert.equal(field('capture.listen').value, '');
    await edit(field('capture.listen'), '0.0.0.0:7380');
    await edit(field('capture.kind'), 'device');
    assert.equal(field('capture.path').value, '/dev/video0');
    await edit(field('capture.kind'), 'file');
    assert.equal(field('capture.path').value, '');
    assert.equal(field('capture.path').labels[0].textContent, 'Video file');
    await edit(field('capture.kind'), 'remote');
    assert.equal(field('capture.listen').value, '0.0.0.0:7380');
    assert.equal(field('output.address').value, 'wled.local');

    await submit(form('Capture'));
    assert.deepEqual(fixture.snapshot().settings.capture, { kind: 'remote', listen: '0.0.0.0:7380' });
    assert.deepEqual(fixture.snapshot().settings.output, output);
    assert.equal(field('output.address').value, 'wled.local');
    assert.equal(badge('Lights'), 'Unsaved changes');

    await click(button('Reset', form('Lights')));
    await edit(field('output.kind'), 'null');
    assert(form('Lights').textContent.includes('There is nothing to configure.'));
    await submit(form('Lights'));
    assert.deepEqual(fixture.snapshot().settings, {
      capture: { kind: 'remote', listen: '0.0.0.0:7380' },
      output: { kind: 'null' },
      processing: defaultProcessing,
    });
    assert.equal($('.device-link a'), null);
    assert.equal(mode('device_native').disabled, true);
  } finally {
    await portal.unmount();
  }
});

test('an unconfigured section starts from its first kind and saves only when valid', async () => {
  const portal = await renderPortal(HeadlessWebPortal, {});
  const { fixture } = portal;
  try {
    assert.equal(badge('Capture'), 'Not configured');
    assert.equal(field('capture.kind').value, 'device');
    assert.equal(errorOf(field('capture.path')), undefined);
    await submit(form('Capture'));
    assert.equal(errorOf(field('capture.path')), 'Enter a value.');
    assert.equal(saves(fixture), 0);
    await edit(field('capture.path'), ' /dev/video0 ');
    await submit(form('Capture'));
    assert.deepEqual(fixture.snapshot().settings.capture, { kind: 'device', path: '/dev/video0' });
    assert.equal(mode('processed').disabled, true);
    assert.equal(hint(mode('processed')), 'Configure your lights first.');
  } finally {
    await portal.unmount();
  }
});

test('the WLED panel edits edge counts and shows the wiring as the renderer orders it', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  const edges = ['top', 'right', 'bottom', 'left'];
  const caption = () => $('.led-layout figcaption').textContent;
  try {
    assert.deepEqual(edges.map((edge) => field(`output.leds.${edge}`).value), ['10', '0', '0', '4']);
    assert.deepEqual(
      [field('output.leds.top').min, field('output.leds.top').max, field('output.leds.top').step],
      ['0', '65535', '1'],
    );
    assert.equal(
      caption(),
      'LED 1 is at the bottom-left corner. Order: left upward: LEDs 1–4; top rightward: LEDs 5–14; right: none; bottom: none.',
    );
    assert.equal($('.total strong').textContent, '14');

    for (const edge of edges) await edit(field(`output.leds.${edge}`), 0);
    assert.equal(caption(), 'No LEDs yet.');
    await submit(form('Lights'));
    assert.equal(
      $('[data-field-message="output.leds"]').textContent,
      'The total must be 1 to 65535 LEDs. Use 0 for an edge without LEDs.',
    );
    assert.equal(document.activeElement, $('[data-field="output.leds"]'));
    await edit(field('output.leds.top'), 70000);
    assert.equal(errorOf(field('output.leds.top')), 'Enter a whole number from 0 to 65535.');
    await edit(field('output.leds.top'), 65535);
    await edit(field('output.leds.left'), 1);
    assert(text().includes('The total must be 1 to 65535 LEDs.'));
    assert.equal(saves(fixture), 0);

    for (const [edge, count] of [['top', 96], ['right', 54], ['bottom', 96], ['left', 0]])
      await edit(field(`output.leds.${edge}`), count);
    assert.equal(
      caption(),
      'LED 1 is at the top-left corner. Order: left: none; top rightward: LEDs 1–96; right downward: LEDs 97–150; bottom leftward: LEDs 151–246.',
    );
    await edit(field('output.address'), ' http://wled.local:8080/ ');
    await submit(form('Lights'));
    assert.deepEqual(fixture.snapshot().settings.output, {
      kind: 'wled',
      address: 'wled.local:8080',
      leds: { top: 96, right: 54, bottom: 96, left: 0 },
    });
    assert.equal(field('output.address').value, 'wled.local:8080');
    assert.equal($('.device-link a').href, 'http://wled.local:8080/');
  } finally {
    await portal.unmount();
  }
});

test('LED runs follow render_edges: bottom-left, clockwise, empty edges skipped', () => {
  assert.deepEqual(
    ledRuns({ top: 3, right: 2, bottom: 3, left: 2 }).map(({ edge, first, last, from }) => [edge, first, last, from]),
    [
      ['left', 1, 2, 'bottom-left'],
      ['top', 3, 5, 'top-left'],
      ['right', 6, 7, 'top-right'],
      ['bottom', 8, 10, 'bottom-right'],
    ],
  );
});

test('fields the forms do not show are preserved on every save', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture: { ...capture, future_option: 1 }, output }, (snapshot) => ({
    settings: {
      ...snapshot.settings,
      colour: { profile: 'warm' },
      processing: { ...snapshot.settings.processing, active_profile: 'ps5', profiles: [{ id: 'ps5' }] },
    },
  }));
  const { fixture } = portal;
  try {
    await edit(field('capture.fps'), 50);
    await submit(form('Capture'));
    await edit(field('output.leds.right'), 6);
    await submit(form('Lights'));
    await edit(field('processing.spatial_reach_percent'), 20);
    await settle();
    assert.deepEqual(fixture.snapshot().settings, {
      capture: { ...capture, future_option: 1, fps: 50 },
      output: { ...output, leds: { ...output.leds, right: 6 } },
      processing: { ...defaultProcessing, spatial_reach_percent: 20, active_profile: 'ps5', profiles: [{ id: 'ps5' }] },
      colour: { profile: 'warm' },
    });
  } finally {
    await portal.unmount();
  }
});

test('the webOS portal edits TV capture within the adapter limits', async () => {
  const portal = await renderPortal(WebosWebPortal, { capture: { kind: 'webos' }, output });
  const { fixture } = portal;
  const input = (name) => field(`capture.${name}`);
  try {
    assert.equal(field('capture.kind'), null);
    assert.equal(field('output.kind'), null);
    assert.equal(form('Capture').querySelector('h2').textContent, 'Capture');
    assert.deepEqual(
      ['width', 'height', 'fps'].map((name) => [input(name).value, input(name).min, input(name).max, input(name).step]),
      [['640', '2', '1920', '2'], ['360', '2', '1080', '2'], ['30', '1', '60', '1']],
    );
    assert.equal(input('include_ui').checked, true);
    await edit(input('width'), 1281);
    assert.equal(errorOf(input('width')), 'Enter an even number from 2 to 1920.');
    await edit(input('width'), 1280);
    await edit(input('height'), 720);
    await edit(input('fps'), 60);
    await click(input('include_ui'));
    await submit(form('Capture'));
    assert.deepEqual(fixture.snapshot().settings.capture, {
      kind: 'webos',
      width: 1280,
      height: 720,
      fps: 60,
      include_ui: false,
    });
    assert.deepEqual(fixture.snapshot().settings.output, output);
  } finally {
    await portal.unmount();
  }
});

test('limits and defaults written in the panels match the generated schemas', async () => {
  const schemas = async (name) =>
    JSON.parse(
      await readFile(new URL(`../packages/contracts/generated/${name}.openapi.json`, import.meta.url), 'utf8'),
    ).components.schemas;
  const application = await schemas('application');
  const desktop = await schemas('host-desktop');
  const webos = await schemas('host-webos');
  const edges = Object.fromEntries(['top', 'right', 'bottom', 'left'].map((edge) => [edge, ledEdgeLimits]));
  for (const [schema, limits, defaults] of [
    [application.Processing, processingLimits, {}],
    [application.CorrectionControls, correctionLimits, {}],
    [application.HueControls, hueLimits, {}],
    [desktop.DeviceSettings, deviceLimits, deviceDefaults],
    [desktop.FileSettings, fileLimits, fileDefaults],
    [webos.WebosSettings, webosLimits, webosDefaults],
    [desktop.LedCounts, edges, {}],
    [webos.LedCounts, edges, {}],
  ]) {
    for (const [name, { min, max }] of Object.entries(limits)) {
      const property = schema.properties[name];
      assert.equal(min, property.minimum, name);
      // Where the schema has no maximum, the panel documents the type bound it uses.
      if (property.maximum !== undefined) assert.equal(max, property.maximum, name);
    }
    for (const [name, value] of Object.entries(defaults))
      assert.equal(value, schema.properties[name].default, name);
  }
  assert.equal(profileLimits.count, application.Processing.properties.profiles.maxItems);
  assert.equal(profileLimits.name, application.ColorProfile.properties.name.maxLength);
  for (const name of Object.keys(application.CorrectionControls.properties).filter((name) => name !== 'hue'))
    assert(name in correctionLimits, `${name} has a control`);
  for (const name of Object.keys(application.HueControls.properties)) assert(name in hueLimits, `${name} has a control`);
});
