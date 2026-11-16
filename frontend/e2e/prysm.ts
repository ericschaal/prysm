/**
 * Fixtures for the portal end-to-end tests: each test gets its own
 * `prysm-headless` on an ephemeral loopback port, with a settings file in a
 * temporary directory, and may start test senders (stand-ins for the TV's
 * slim worker) that stream generated frames to it. Nothing here talks to
 * anything but loopback.
 *
 * A test that sets `devices` runs `prysm-headless`'s `fake_devices` example
 * instead, which finds those video devices and answers their modes.
 *
 * The binaries come from cargo (see docs/developers/frontend.md):
 * `PRYSM_HEADLESS_BIN`, `PRYSM_FAKE_DEVICES_BIN` and `PRYSM_TEST_SENDER_BIN`,
 * or else the debug builds in `$CARGO_TARGET_DIR` (default `target`).
 */
import { test as base, expect, type Locator } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';

const root = path.resolve(import.meta.dirname, '../..');
const target = process.env.CARGO_TARGET_DIR ?? path.join(root, 'target');
const headlessBin =
  process.env.PRYSM_HEADLESS_BIN ??
  path.join(target, 'debug', 'prysm-headless');
const fakeDevicesBin =
  process.env.PRYSM_FAKE_DEVICES_BIN ??
  path.join(target, 'debug', 'examples', 'fake_devices');
const dist = path.join(root, 'frontend', 'dist', 'headless', 'web-portal');
const senderBin =
  process.env.PRYSM_TEST_SENDER_BIN ??
  path.join(target, 'debug', 'examples', 'test_sender');

/** The longest any start, line or exit is waited for. */
const BOUND_MS = 10_000;

/** A small raw picture: no encoder, little CPU. */
export const remoteCapture = {
  kind: 'remote',
  width: 32,
  height: 18,
  fps: 30,
  encoding: 'raw',
};

/** What a test's settings file holds before Prysm starts. */
export type Settings = Record<string, unknown>;

/** A span of sizes or rates, as `POST /api/v1/capture/modes` answers. */
export interface Span {
  min: number;
  max: number;
  step: number;
}

/** One size and rate a capture delivers, each a span. */
export interface Mode {
  width: Span;
  height: Span;
  fps: Span;
}

/** One size at one rate. */
export const fixedMode = (width: number, height: number, fps: number) => ({
  width: { min: width, max: width, step: 1 },
  height: { min: height, max: height, step: 1 },
  fps: { min: fps, max: fps, step: 1 },
});

/** Every even size up to `width` × `height`, at 1 to `fps` fps. */
export const anyMode = (width: number, height: number, fps: number) => ({
  width: { min: 2, max: width, step: 2 },
  height: { min: 2, max: height, step: 2 },
  fps: { min: 1, max: fps, step: 1 },
});

/** A stand-in video device: found by discovery, never captured from. */
export interface FakeDevice {
  name: string;
  path: string;
  modes: Mode[];
}

/** The parts of `GET /api/v1/state` the tests read. */
export interface State {
  target: string;
  session: { state: string };
  settings: { processing: { brightness_percent: number } };
  failure: { message: string } | null;
}

/** A child process whose output lines can be awaited. */
class Process {
  readonly lines: string[] = [];
  private readonly child: ChildProcess;
  private readonly changed = new Set<() => void>();
  private failure: Error | undefined;
  private readonly exited: Promise<number | null>;

  constructor(
    command: string,
    args: string[],
    env: Record<string, string> = {},
  ) {
    this.child = spawn(command, args, {
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, ...env },
    });
    for (const stream of [this.child.stdout, this.child.stderr]) {
      if (!stream) continue;
      createInterface({ input: stream }).on('line', (line) => {
        this.lines.push(line);
        this.notify();
      });
    }
    this.exited = new Promise((resolve) => {
      this.child.on('error', (error) => {
        this.failure = new Error(
          `cannot start ${command} (build it first: docs/developers/frontend.md): ${error.message}`,
        );
        this.notify();
        resolve(null);
      });
      this.child.on('exit', (code) => {
        this.failure ??= new Error(`${command} exited (${code})`);
        this.notify();
        resolve(code);
      });
    });
  }

  private notify() {
    for (const listener of this.changed) listener();
  }

  /** The first output line (from now or earlier) containing `needle`. */
  waitFor(needle: string): Promise<string> {
    return new Promise((resolve, reject) => {
      const check = () => {
        const line = this.lines.find((each) => each.includes(needle));
        const failure = this.failure;
        if (line !== undefined) done(() => resolve(line));
        else if (failure) done(() => reject(failure));
      };
      const timer = setTimeout(
        () =>
          done(() =>
            reject(
              new Error(
                `no "${needle}" within ${BOUND_MS} ms:\n${this.lines.join('\n')}`,
              ),
            ),
          ),
        BOUND_MS,
      );
      const done = (settle: () => void) => {
        clearTimeout(timer);
        this.changed.delete(check);
        settle();
      };
      this.changed.add(check);
      check();
    });
  }

  /** SIGTERM, then the exit code (`null`: ended by the signal). */
  async stop(): Promise<number | null> {
    if (this.child.exitCode !== null || this.child.signalCode !== null)
      return this.exited;
    this.child.kill('SIGTERM');
    let timer: NodeJS.Timeout | undefined;
    const late = new Promise<'late'>((resolve) => {
      timer = setTimeout(() => resolve('late'), BOUND_MS);
    });
    const code = await Promise.race([this.exited, late]);
    clearTimeout(timer);
    if (code === 'late') {
      this.child.kill('SIGKILL');
      throw new Error(`no exit within ${BOUND_MS} ms of SIGTERM`);
    }
    return code;
  }
}

/** The host to start: the production binary, or one with fake devices. */
function host(devices: FakeDevice[] | null, args: string[]) {
  return devices === null
    ? new Process(headlessBin, args)
    : new Process(fakeDevicesBin, args, {
        PRYSM_FAKE_DEVICES: JSON.stringify(devices),
      });
}

/**
 * The entry script a page loads, named by its content, e.g.
 * `/assets/index-CZ3hLPcm.js`.
 */
export const entryScript = /\/assets\/index-[\w-]+\.js/;

/**
 * Fails unless the `prysm-headless` under test and its `fake_devices`
 * example serve a real bundle, and the one `npm run bundle` last wrote if
 * `dist` has one: they embed the bundle when cargo builds them, so one
 * built before the frontend serves the old one, and a debug build without
 * it serves only a "not built" page.
 */
export async function checkBundle(): Promise<void> {
  for (const devices of [null, []]) await checkHost(devices);
}

async function checkHost(devices: FakeDevice[] | null): Promise<void> {
  // No bundle here when the binary was built elsewhere (CI hands it to this
  // job): it must still serve one, but there is nothing to compare it with.
  const assets = await readFile(path.join(dist, 'assets.rs'), 'utf8').catch(
    () => null,
  );
  const built = assets === null ? null : entryScript.exec(assets)?.[0];
  const dir = await mkdtemp(path.join(tmpdir(), 'prysm-e2e-'));
  const file = path.join(dir, 'settings.json');
  await writeFile(file, JSON.stringify({ output: { kind: 'null' } }));
  const checked = host(devices, [file, '--listen', '127.0.0.1:0']);
  const bin = devices === null ? headlessBin : fakeDevicesBin;
  try {
    const line = await checked.waitFor('portal http://');
    const url = /http:\/\/[^/\s]+/.exec(line)?.[0];
    if (!url) throw new Error(`no address in ${line}`);
    const served = entryScript.exec(await (await fetch(`${url}/`)).text())?.[0];
    if (!served)
      throw new Error(
        `${bin} serves no portal bundle (built without frontend/dist): run npm run bundle, then rebuild it (cargo build -p prysm-headless --bins --example fake_devices)`,
      );
    if (assets !== null && served !== built)
      throw new Error(
        `${bin} serves ${served ?? 'no script'}, but dist has ${built ?? 'none'}: rebuild it (cargo build -p prysm-headless --bins --example fake_devices) after npm run bundle`,
      );
  } finally {
    await checked.stop();
    await rm(dir, { recursive: true, force: true });
  }
}

export interface Prysm {
  /** The portal's origin, e.g. `http://127.0.0.1:53211`. */
  url: string;
  /** The engine's state, read straight from the API. */
  state(): Promise<State>;
  /** The settings file as it is on disk now. */
  savedSettings(): Promise<Settings>;
  /** Stop Prysm and start it again at the same address, as an update does. */
  restart(): Promise<void>;
}

export interface Sender {
  /** The first output line containing `needle`: `connected`, `refused`. */
  waitFor(needle: string): Promise<string>;
}

export const test = base.extend<{
  /** The settings file's content. Override with `test.use({ settings })`. */
  settings: Settings;
  /**
   * The video devices Prysm finds, with their modes; `null` (the default):
   * the production binary, which looks for real ones.
   */
  devices: FakeDevice[] | null;
  prysm: Prysm;
  /** Start a test sender streaming to this test's Prysm (stopped after the test). */
  startSender: (...args: string[]) => Sender;
}>({
  settings: [
    { capture: remoteCapture, output: { kind: 'null' } },
    { option: true },
  ],
  devices: [null, { option: true }],

  prysm: async ({ settings, devices }, provide, testInfo) => {
    const dir = await mkdtemp(path.join(tmpdir(), 'prysm-e2e-'));
    const file = path.join(dir, 'settings.json');
    await writeFile(file, JSON.stringify(settings));
    let running = host(devices, [file, '--listen', '127.0.0.1:0']);
    const logs: string[] = [];
    let code: number | null;
    try {
      const line = await running.waitFor('portal http://');
      const url = /http:\/\/[^/\s]+/.exec(line)?.[0];
      if (!url) throw new Error(`no address in ${line}`);
      await provide({
        url,
        savedSettings: async () =>
          JSON.parse(await readFile(file, 'utf8')) as Settings,
        state: async () => {
          const response = await fetch(`${url}/api/v1/state`);
          expect(response.status).toBe(200);
          return (await response.json()) as State;
        },
        restart: async () => {
          expect(await running.stop(), 'exits cleanly before a restart').toBe(
            0,
          );
          logs.push(...running.lines, '(restart)');
          running = host(devices, [file, '--listen', new URL(url).host]);
          await running.waitFor('portal http://');
        },
      });
    } finally {
      code = await running.stop();
      await testInfo.attach('prysm-headless.log', {
        body: [...logs, ...running.lines].join('\n'),
        contentType: 'text/plain',
      });
      await rm(dir, { recursive: true, force: true });
    }
    expect(code, 'prysm-headless exits cleanly on SIGTERM').toBe(0);
  },

  baseURL: async ({ prysm }, provide) => provide(prysm.url),

  startSender: async ({ prysm }, provide) => {
    const started: Process[] = [];
    await provide((...args) => {
      const sender = new Process(senderBin, [
        `${prysm.url.replace('http:', 'ws:')}/api/v1/capture/stream`,
        ...args,
      ]);
      started.push(sender);
      return { waitFor: (needle) => sender.waitFor(needle) };
    });
    for (const sender of started) await sender.stop();
  },
});

/**
 * Tick a kind in a form's kind rows (`Capture from`, `Send colours to`):
 * a row is clicked as a whole, its radio drawn as the tick.
 */
export async function chooseKind(
  form: Locator,
  heading: string,
  label: string,
): Promise<void> {
  await form
    .getByRole('radiogroup', { name: heading })
    .getByText(label, { exact: true })
    .click();
}

export { expect };
