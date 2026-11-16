import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import {
  modesRetryMs,
  offeredPresets,
  picturePresets,
  resolve,
} from '@prysm/web-portal';
import { typedDeviceMs } from '@prysm/capture-device-ui';
import {
  anyMode,
  fakeCaptureModesClient,
  fixedMode,
} from '../testkit/capture-modes';
import { fakeDiscoveryClient } from '../testkit/discovery';
import {
  $,
  $$,
  button,
  click,
  edit,
  field,
  form,
  openEditor,
  radio,
  renderPortal,
  sleep,
  submit,
} from './dom';

const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const path = '/dev/video0';
const known = (...modes) => ({ status: 'known', modes });
const unknown = { status: 'unknown' };
const plain = (text) => text.replace(/\u00a0/g, ' ');
/** The offered presets as `name size · rate`. */
const offered = (modes) =>
  offeredPresets(modes).map(
    ({ preset, width, height, fps }) => `${preset} ${width}×${height}@${fps}`,
  );

const choice = () => $('.picture-choice', form('Video source'));
/** The size-and-rate rows: label, facts, ticked, locked. */
const rows = () =>
  $$('.row-choice', choice()).map((row) => {
    const input = $('input', row);
    return [
      $('.row-label', row).textContent,
      plain($('.row-heading .row-value', row)?.textContent ?? ''),
      input.checked,
      input.disabled,
    ];
  });
/** The one line above the rows, if any. */
const line = () =>
  plain($('.picture-choice > .field-note > .help')?.textContent ?? '');
const saveButton = () => button('Use this video source', form('Video source'));
const settled = () => act(async () => sleep(0));

test('a preset is its target where a mode takes it, else the smallest 16:9 mode that covers it at its rate', () => {
  const span = (min, max, step = 1) => ({ min, max, step });
  // Ranges that take the targets: the targets, all three.
  assert.deepEqual(offered([anyMode(1920, 1080, 60)]), [
    'balanced 640×360@30',
    'smooth 640×360@60',
    'efficient 320×180@30',
  ]);
  // A card's fixed sizes: the smallest wide one at the rate; Efficient
  // comes out as Balanced and shows once, under Balanced.
  const card = [
    fixedMode(640, 480, 30),
    fixedMode(1280, 720, 30),
    fixedMode(1280, 720, 60),
    fixedMode(1920, 1080, 30),
  ];
  assert.deepEqual(offered(card), [
    'balanced 1280×720@30',
    'smooth 1280×720@60',
  ]);
  // No wide size: any shape that covers it.
  assert.deepEqual(
    offered([fixedMode(800, 600, 30), fixedMode(640, 480, 30)]),
    ['balanced 640×480@30'],
  );
  // The rate is never rounded: a 60 fps-only card offers only Smooth.
  assert.deepEqual(offered([fixedMode(1920, 1080, 60)]), [
    'smooth 1920×1080@60',
  ]);
  // A range axis rounds up to its next step; a rate step must hit the target.
  assert.deepEqual(
    resolve(
      [
        {
          width: span(16, 1920, 16),
          height: span(16, 1080, 16),
          fps: span(15, 60, 15),
        },
      ],
      picturePresets.balanced,
    ),
    { width: 640, height: 368, fps: 30 },
  );
  assert.equal(
    resolve(
      [
        {
          width: span(2, 1920, 2),
          height: span(2, 1080, 2),
          fps: span(25, 50, 25),
        },
      ],
      picturePresets.balanced,
    ),
    null,
  );
  // Too small for the target, or nothing at all: nothing offered.
  assert.deepEqual(offered([fixedMode(320, 240, 30)]), [
    'efficient 320×240@30',
  ]);
  assert.deepEqual(offered([]), []);
  // The TV's video chip streams up to 30 fps: no Smooth.
  assert.deepEqual(offered([anyMode(1920, 1088, 30)]), [
    'balanced 640×360@30',
    'efficient 320×180@30',
  ]);
});

test('a device’s own modes decide the presets and what they set; a deliverable saved choice is left alone', async () => {
  const modes = fakeCaptureModesClient(() =>
    known(fixedMode(1280, 720, 30), fixedMode(1280, 720, 60)),
  );
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path, width: 1280, height: 720 }, output },
    undefined,
    { captureModes: modes.client },
  );
  try {
    await openEditor('Video source');
    assert.deepEqual(modes.calls, [{ kind: 'device', path }]);
    assert.deepEqual(rows(), [
      ['Balanced', '1280 × 720 · 30 fps', true, false],
      ['Smooth', '1280 × 720 · 60 fps', false, false],
    ]);
    assert.equal(line(), '');
    // Nothing to say: the line takes no room, but stays to be announced.
    const note = $('.picture-choice > .field-note');
    assert.equal(note.dataset.said, 'false');
    assert.equal(note.getAttribute('role'), 'status');
    assert(!saveButton(), 'nothing changed');
    await click(radio('Smooth'));
    await submit(form('Video source'));
    assert.deepEqual(portal.fixture.snapshot().settings.capture, {
      kind: 'device',
      path,
      width: 1280,
      height: 720,
      fps: 60,
    });
  } finally {
    await portal.unmount();
  }
});

test('a saved size and rate no preset sets shows as one ticked Custom row until a preset replaces it', async () => {
  const modes = fakeCaptureModesClient(() =>
    known(fixedMode(800, 600, 25), fixedMode(640, 360, 30)),
  );
  const portal = await renderPortal(
    HeadlessWebPortal,
    {
      capture: { kind: 'device', path, width: 800, height: 600, fps: 25 },
      output,
    },
    undefined,
    { captureModes: modes.client },
  );
  try {
    await openEditor('Video source');
    assert.deepEqual(rows(), [
      ['Balanced', '640 × 360 · 30 fps', false, false],
      ['Custom', '800 × 600 · 25 fps', true, false],
    ]);
    // Read as chosen, with what it means; nothing else is offered for it.
    const custom = radio('Custom');
    const described = custom
      .getAttribute('aria-describedby')
      .split(' ')
      .map((id) => plain(document.getElementById(id).textContent));
    assert.deepEqual(described, [
      'Kept until you choose another.',
      '800 × 600 · 25 fps',
    ]);
    assert(!saveButton());
    await click(radio('Balanced'));
    assert.deepEqual(rows(), [['Balanced', '640 × 360 · 30 fps', true, false]]);
    assert(saveButton());
  } finally {
    await portal.unmount();
  }
});

test('a saved size and rate the device cannot deliver moves to Balanced, and the line says why', async () => {
  const modes = fakeCaptureModesClient(() => known(fixedMode(1280, 720, 30)));
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path, fps: 60 }, output },
    undefined,
    { captureModes: modes.client },
  );
  try {
    await openEditor('Video source');
    assert.deepEqual(rows(), [
      ['Balanced', '1280 × 720 · 30 fps', true, false],
    ]);
    assert.equal(
      line(),
      'This device can’t do 640 × 360 at 60 fps, so it is set to Balanced.',
    );
    assert.equal($('.picture-choice > .field-note').dataset.said, 'true');
    // An unsaved change, saved the usual way.
    await submit(form('Video source'));
    assert.deepEqual(portal.fixture.snapshot().settings.capture, {
      kind: 'device',
      path,
      width: 1280,
      height: 720,
      fps: 30,
    });
  } finally {
    await portal.unmount();
  }
});

test('the devices present are rows to tick; a saved one not found keeps its row, not plugged in, and locks the sizes', async () => {
  const discovery = fakeDiscoveryClient({
    'capture/device': [
      {
        label: 'USB Video',
        detail: '/dev/video2',
        fields: { path: '/dev/video2' },
      },
    ],
  });
  const modes = fakeCaptureModesClient((capture) =>
    capture.path === '/dev/video2' ? known(anyMode(1920, 1080, 60)) : unknown,
  );
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path, fps: 60 }, output },
    undefined,
    { captureModes: modes.client, discovery: discovery.client },
  );
  const devices = () =>
    $$('.device-rows .row-choice').map((row) => [
      $('.row-label', row).textContent,
      $('.row-value', row)?.textContent,
      $('input', row).checked,
    ]);
  try {
    await openEditor('Video source');
    await settled();
    // Looked for as the editor opened, nothing pressed.
    assert.deepEqual(devices(), [
      // Named devices without their paths; the missing one by its path.
      ['USB Video', undefined, false],
      [path, 'Not plugged in', true],
    ]);
    assert.equal(
      $('.device-rows').getAttribute('role'),
      'radiogroup',
      'one choice',
    );
    // Its size and rate stay as saved, locked, and the line says why.
    assert.deepEqual(rows(), [
      ['Balanced', '640 × 360 · 30 fps', false, true],
      ['Smooth', '640 × 360 · 60 fps', true, true],
      ['Efficient', '320 × 180 · 30 fps', false, true],
    ]);
    assert.equal(line(), 'Plug in the device to choose its size and rate.');
    // Typing waits under Advanced, by its own name.
    assert.equal(
      field('capture.path').labels[0].textContent,
      'Enter the device by hand',
    );
    assert(field('capture.path').closest('[hidden]'));

    await click($('.device-rows input'));
    await settled();
    assert.equal(field('capture.path').value, '/dev/video2');
    assert.deepEqual(devices(), [['USB Video', undefined, true]]);
    assert.deepEqual(
      rows().map(([label, , checked, locked]) => [label, checked, locked]),
      [
        ['Balanced', false, false],
        ['Smooth', true, false],
        ['Efficient', false, false],
      ],
    );
    assert.equal(line(), '');

    const looks = () =>
      discovery.calls.filter((call) => call === 'capture/device').length;
    const before = looks();
    await click(button('Look again'));
    assert.equal(looks(), before + 1);
  } finally {
    await portal.unmount();
  }
});

test('nothing found shows the typed field as it is; a device that delivers nothing says so under the devices, without sizes', async () => {
  for (const [found, answer, said, sizes] of [
    [
      [],
      known(anyMode(1920, 1080, 60)),
      /^No devices found\. Plug one in, or enter it below\.$/,
      true,
    ],
    [
      [{ label: 'Old grabber', detail: path, fields: { path } }],
      known(),
      /^Prysm can’t capture from this device\.$/,
      false,
    ],
  ]) {
    const discovery = fakeDiscoveryClient({ 'capture/device': found });
    const modes = fakeCaptureModesClient(() => answer);
    const portal = await renderPortal(
      HeadlessWebPortal,
      { capture: { kind: 'device', path }, output },
      undefined,
      { captureModes: modes.client, discovery: discovery.client },
    );
    try {
      await openEditor('Video source');
      await settled();
      assert.match($('.device-rows [role="status"]').textContent, said);
      assert.equal(
        !field('capture.path').closest('[hidden]'),
        found.length === 0,
        'typed in plain sight only when nothing was found',
      );
      assert.equal(Boolean(choice()), sizes);
    } finally {
      await portal.unmount();
    }
  }
});

test('a typed device is asked about once typing pauses', async () => {
  const modes = fakeCaptureModesClient();
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path }, output },
    undefined,
    { captureModes: modes.client },
  );
  try {
    await openEditor('Video source');
    await edit(field('capture.path'), '/dev/vid');
    await edit(field('capture.path'), '/dev/video3');
    assert.equal(modes.calls.length, 1, 'not yet');
    await act(async () => sleep(typedDeviceMs + 50));
    assert.deepEqual(modes.calls.at(-1), {
      kind: 'device',
      path: '/dev/video3',
    });
    assert.equal(modes.calls.length, 2, 'once, for what was typed last');
  } finally {
    await portal.unmount();
  }
});

test('a chosen preset follows to another device’s size for it, without a line', async () => {
  const discovery = fakeDiscoveryClient({
    'capture/device': [
      { label: 'Card', detail: '/dev/video0', fields: { path: '/dev/video0' } },
      {
        label: 'Webcam',
        detail: '/dev/video1',
        fields: { path: '/dev/video1' },
      },
    ],
  });
  const modes = fakeCaptureModesClient((capture) =>
    capture.path === '/dev/video0'
      ? known(fixedMode(1280, 720, 30))
      : known(anyMode(1920, 1080, 60)),
  );
  const portal = await renderPortal(
    HeadlessWebPortal,
    {
      capture: { kind: 'device', path, width: 1280, height: 720, fps: 30 },
      output,
    },
    undefined,
    { captureModes: modes.client, discovery: discovery.client },
  );
  try {
    await openEditor('Video source');
    await settled();
    assert.deepEqual(rows()[0], [
      'Balanced',
      '1280 × 720 · 30 fps',
      true,
      false,
    ]);
    await click($$('.device-rows input')[1]);
    await settled();
    assert.deepEqual(rows()[0], [
      'Balanced',
      '640 × 360 · 30 fps',
      true,
      false,
    ]);
    assert.equal(line(), '');
  } finally {
    await portal.unmount();
  }
});

test('while the first answer is coming the rows stay on the saved choice, locked, without a line', async () => {
  const waiting = [];
  const client = {
    modes: () => new Promise((resolved) => waiting.push(resolved)),
  };
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'remote', fps: 60, width: 640, height: 360 }, output },
    undefined,
    { captureModes: client },
  );
  try {
    await openEditor('Video source');
    assert.deepEqual(
      rows().map(([label, , checked, locked]) => [label, checked, locked]),
      [
        ['Balanced', false, true],
        ['Smooth', true, true],
        ['Efficient', false, true],
      ],
    );
    assert.equal(line(), '');
    await act(async () => {
      for (const answer of waiting) answer(known(anyMode(1920, 1080, 60)));
    });
    await settled();
    assert.deepEqual(
      rows().map(([, , , locked]) => locked),
      [false, false, false],
    );
  } finally {
    await portal.unmount();
  }
});

test('a lookup that fails says so and is asked again', async (t) => {
  const modes = fakeCaptureModesClient();
  modes.refuseNext({
    kind: 'unavailable',
    message: 'The portal did not answer.',
  });
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path }, output },
    undefined,
    { captureModes: modes.client },
  );
  try {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    await openEditor('Video source');
    assert.equal(
      line(),
      'Prysm didn’t say what this source can do. Asking again…',
    );
    await act(async () => t.mock.timers.tick(modesRetryMs));
    t.mock.timers.reset();
    await settled();
    assert.equal(line(), '');
    assert.equal(radio('Balanced').disabled, false);
  } finally {
    await portal.unmount();
  }
});

test('a capture-only TV that has not connected locks the rows on the saved choice and offers every encoding; its offer then decides', async (t) => {
  let offer = () => unknown;
  const modes = fakeCaptureModesClient((capture) => offer(capture));
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'remote', width: 640, height: 360, fps: 60 }, output },
    undefined,
    { captureModes: modes.client },
  );
  const encodings = () =>
    $$('option', field('capture.encoding')).map((option) => option.value);
  try {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    await openEditor('Video source');
    assert.deepEqual(
      modes.calls.map((call) => call.encoding),
      ['h264_hardware', 'h264_software', 'raw'],
    );
    assert.equal(
      line(),
      'The TV isn’t connected. Give the TV app the address of this page.',
    );
    assert.deepEqual(
      rows().map(([label, , checked, locked]) => [label, checked, locked]),
      [
        ['Balanced', false, true],
        ['Smooth', true, true],
        ['Efficient', false, true],
      ],
    );
    assert.deepEqual(encodings(), ['h264_hardware', 'h264_software', 'raw']);

    // The TV announces: its video chip streams up to 30 fps, the processor
    // up to 60, and it sends nothing uncompressed. Asked again by itself.
    offer = ({ encoding }) =>
      ({
        h264_hardware: known(anyMode(1920, 1088, 30)),
        h264_software: known(anyMode(1920, 1080, 60)),
        raw: known(),
      })[encoding];
    await act(async () => t.mock.timers.tick(modesRetryMs));
    t.mock.timers.reset();
    await settled();
    assert.deepEqual(
      rows().map(([label, facts, checked]) => [label, facts, checked]),
      [
        ['Balanced', '640 × 360 · 30 fps', true],
        ['Efficient', '320 × 180 · 30 fps', false],
      ],
      'no Smooth with the video chip',
    );
    assert.equal(
      line(),
      'This TV can’t do 640 × 360 at 60 fps, so it is set to Balanced.',
    );
    assert.deepEqual(encodings(), ['h264_hardware', 'h264_software']);
    assert(field('capture.encoding').closest('[hidden]'), 'under Advanced');

    // Another encoding: its own presets, and the page asks again on focus.
    await edit(field('capture.encoding'), 'h264_software');
    assert.deepEqual(
      rows().map(([label]) => label),
      ['Balanced', 'Smooth', 'Efficient'],
    );
    const asked = modes.calls.length;
    await act(async () => window.dispatchEvent(new window.Event('focus')));
    await settled();
    assert.equal(modes.calls.length, asked + 3);
  } finally {
    await portal.unmount();
  }
});

test('a saved encoding the TV does not offer stays in sight with the reason, without sizes', async () => {
  const modes = fakeCaptureModesClient(({ encoding }) =>
    encoding === 'raw' ? known() : known(anyMode(1920, 1080, 30)),
  );
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'remote', encoding: 'raw' }, output },
    undefined,
    { captureModes: modes.client },
  );
  const encoding = () => field('capture.encoding');
  try {
    await openEditor('Video source');
    await settled();
    assert(!encoding().closest('[hidden]'), 'not under Advanced');
    assert.equal(
      encoding().closest('.field').querySelector('.help').textContent,
      'This TV can’t send the picture this way.',
    );
    assert.equal(
      encoding().closest('.field').querySelector('.help').dataset.tone,
      'warning',
    );
    assert(!choice());
    assert.deepEqual(
      $$('option', encoding()).map((option) => option.value),
      ['h264_hardware', 'h264_software', 'raw'],
    );
    await edit(encoding(), 'h264_hardware');
    assert.deepEqual(
      rows().map(([label]) => label),
      ['Balanced', 'Efficient'],
    );
    assert(!encoding().closest('[hidden]'), 'it stays where it is');
    // Said as a problem, but not a check: another encoding saves at once.
    await submit(form('Video source'));
    assert.equal(
      portal.fixture.snapshot().settings.capture.encoding,
      'h264_hardware',
    );
  } finally {
    await portal.unmount();
  }
});

test('a saved device that is not found keeps the name this browser last saw for it', async () => {
  const memory = {};
  // A storage that outlives the page, as a browser's does.
  const withStorage = (snapshot) => {
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: (key) => memory[key] ?? null,
        setItem: (key, value) => {
          memory[key] = String(value);
        },
        removeItem: (key) => {
          delete memory[key];
        },
      },
    });
    return snapshot;
  };
  const card = { label: 'Elgato Cam Link 4K', detail: path, fields: { path } };
  const visit = async (found, answer) => {
    const portal = await renderPortal(
      HeadlessWebPortal,
      { capture: { kind: 'device', path }, output },
      withStorage,
      {
        captureModes: fakeCaptureModesClient(() => answer).client,
        discovery: fakeDiscoveryClient({ 'capture/device': found }).client,
      },
    );
    await openEditor('Video source');
    await settled();
    return portal;
  };
  // Seen once, plugged in.
  let portal = await visit([card], known(anyMode(1920, 1080, 60)));
  await portal.unmount();
  // Unplugged on the next visit: its name, its path, and why, in the
  // problem colour.
  portal = await visit([], unknown);
  try {
    const row = $('.device-rows .row-choice');
    assert.equal($('.row-label', row).textContent, 'Elgato Cam Link 4K');
    assert.equal($('.row-value', row).textContent, `${path} · Not plugged in`);
    assert.equal($('.row-problem', row).textContent, 'Not plugged in');
    assert.equal($('input', row).checked, true);
  } finally {
    await portal.unmount();
  }
});

test('with devices to pick, no device chosen is said at the rows, which take the focus; Advanced stays closed', async () => {
  const discovery = fakeDiscoveryClient({
    'capture/device': [
      {
        label: 'USB Video',
        detail: '/dev/video2',
        fields: { path: '/dev/video2' },
      },
    ],
  });
  const portal = await renderPortal(HeadlessWebPortal, {}, undefined, {
    discovery: discovery.client,
  });
  const wizard = () => $('form.setup');
  try {
    await settled();
    await submit(wizard());
    const devices = $('.device-rows');
    assert.equal(document.activeElement, devices);
    assert.equal(devices.getAttribute('aria-invalid'), 'true');
    assert.equal(
      document.getElementById(devices.getAttribute('aria-describedby'))
        .textContent,
      'Choose a device, or enter one by hand.',
    );
    assert.equal(
      button('Advanced', wizard()).getAttribute('aria-expanded'),
      'false',
    );
    await click($('input', devices));
    assert.equal(devices.getAttribute('aria-invalid'), null);
  } finally {
    await portal.unmount();
  }
});

test('Look again keeps the focus while looking and ignores presses meanwhile', async () => {
  const card = { label: 'Card', detail: path, fields: { path } };
  let hold = false;
  let release;
  let looks = 0;
  const discovery = {
    candidates: (section) => {
      if (section === 'capture') looks += 1;
      return hold
        ? new Promise((resolve) => {
            release = () => resolve([card]);
          })
        : Promise.resolve([card]);
    },
  };
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path }, output },
    undefined,
    { discovery },
  );
  try {
    await openEditor('Video source');
    await settled();
    hold = true;
    const again = button('Look again');
    again.focus();
    const before = looks;
    await click(again);
    assert.equal(document.activeElement, again, 'still focused');
    assert.equal(again.disabled, false);
    assert.equal(again.getAttribute('aria-busy'), 'true');
    assert.equal(again.getAttribute('aria-disabled'), 'true');
    assert.equal(
      $('.device-rows [role="status"]').textContent,
      'Looking for devices…',
    );
    await click(again);
    assert.equal(looks, before + 1, 'one look at a time');
    await act(async () => release());
    await settled();
    assert.equal(again.getAttribute('aria-busy'), 'false');
    assert.equal(document.activeElement, again);
  } finally {
    await portal.unmount();
  }
});

test('the first lookup lists nothing until it answers, then each device once, in place; looking again keeps the rows', async () => {
  const card = { label: 'Card', detail: path, fields: { path } };
  const webcam = {
    label: 'Webcam',
    detail: '/dev/video1',
    fields: { path: '/dev/video1' },
  };
  const waiting = [];
  const discovery = {
    candidates: (section) =>
      section === 'capture'
        ? new Promise((resolve) => waiting.push(() => resolve([webcam, card])))
        : Promise.resolve([]),
  };
  const answer = () =>
    act(async () => {
      for (const resolve of waiting.splice(0)) resolve();
    });
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path }, output },
    undefined,
    { discovery },
  );
  const devices = () => $$('.device-rows .row-choice');
  try {
    await openEditor('Video source');
    // Looking: no row yet, not even the saved device's.
    assert.equal(devices().length, 0);
    assert.equal(
      $('.device-rows [role="status"]').textContent,
      'Looking for devices…',
    );
    await answer();
    await settled();
    assert.deepEqual(
      devices().map((row) => $('.row-label', row).textContent),
      ['Webcam', 'Card'],
    );
    const saved = devices()[1];
    assert.equal($('input', saved).checked, true);
    // Looking again: the same rows, the same nodes, until and after it answers.
    await click(button('Look again'));
    assert.equal(devices()[1], saved);
    assert.equal($('.row-label', saved).textContent, 'Card');
    await answer();
    await settled();
    assert.equal(devices()[1], saved);
    assert.equal($('.row-label', saved).textContent, 'Card');
  } finally {
    await portal.unmount();
  }
});

test('device rows say where a device is only when its name does not tell it apart', async () => {
  const discovery = fakeDiscoveryClient({
    'capture/device': [
      {
        label: 'Elgato',
        detail: '/dev/video7',
        fields: { path: '/dev/video7' },
      },
      {
        label: 'USB Video',
        detail: '/dev/video8',
        fields: { path: '/dev/video8' },
      },
      {
        label: 'USB Video',
        detail: '/dev/video9',
        fields: { path: '/dev/video9' },
      },
      { label: '', detail: '/dev/video3', fields: { path: '/dev/video3' } },
      {
        label: 'Capture box',
        detail: 'Front HDMI',
        fields: { path: '/dev/video4' },
      },
    ],
  });
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path: '/dev/video7' }, output },
    undefined,
    { discovery: discovery.client },
  );
  try {
    await openEditor('Video source');
    await settled();
    assert.deepEqual(
      $$('.device-rows .row-choice').map((row) => [
        $('.row-label', row).textContent,
        $('.row-value', row)?.textContent,
      ]),
      [
        ['Elgato', undefined],
        ['USB Video', '/dev/video8'],
        ['USB Video', '/dev/video9'],
        ['/dev/video3', undefined],
        ['Capture box', 'Front HDMI'],
      ],
    );
  } finally {
    await portal.unmount();
  }
});

test('Look again sits on the devices heading, outside the group name', async () => {
  const discovery = fakeDiscoveryClient({
    'capture/device': [{ label: 'Elgato', detail: path, fields: { path } }],
  });
  const portal = await renderPortal(
    HeadlessWebPortal,
    { capture: { kind: 'device', path }, output },
    undefined,
    { discovery: discovery.client },
  );
  try {
    await openEditor('Video source');
    await settled();
    const group = $('.device-rows');
    assert.equal(
      document.getElementById(group.getAttribute('aria-labelledby'))
        .textContent,
      'Device',
    );
    assert.equal(button('Look again').closest('legend'), $('legend', group));
    // Nothing to say under the rows: no line.
    assert.equal($('.device-rows > .help').textContent, '');
  } finally {
    await portal.unmount();
  }
});

test('a preset above its target says it is the smallest the source offers at its rate', async () => {
  for (const [capture, source] of [
    [{ kind: 'device', path }, 'this device'],
    [{ kind: 'remote' }, 'this TV'],
  ]) {
    const modes = fakeCaptureModesClient(() =>
      known(fixedMode(1280, 720, 30), fixedMode(1280, 720, 60)),
    );
    const portal = await renderPortal(
      HeadlessWebPortal,
      { capture: { ...capture, width: 1280, height: 720, fps: 30 }, output },
      undefined,
      { captureModes: modes.client },
    );
    const about = (label) =>
      plain(
        $('.row-text > .row-value', radio(label).closest('.row'))
          ?.textContent ?? '',
      );
    try {
      await openEditor('Video source');
      assert.equal(
        about('Balanced'),
        `The smallest size ${source} offers at 30 fps.`,
      );
      await click(radio('Smooth'));
      assert.equal(
        about('Smooth'),
        `The smallest size ${source} offers at 60 fps.`,
      );
    } finally {
      await portal.unmount();
    }
  }
});
