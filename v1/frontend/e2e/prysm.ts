/**
 * Fixtures for the portal end-to-end tests: each test gets its own
 * `prysm-headless` on an ephemeral loopback port, with a settings file in a
 * temporary directory, and may start test senders (stand-ins for the TV's
 * slim worker) that stream generated frames to it. Nothing here talks to
 * anything but loopback.
 *
 * The binaries come from cargo (see docs/developers/frontend.md):
 * `PRYSM_HEADLESS_BIN` and `PRYSM_TEST_SENDER_BIN`, or else the debug builds
 * in `$CARGO_TARGET_DIR` (default `v1/target`).
 */
import { test as base, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';

const v1 = path.resolve(import.meta.dirname, '../..');
const target = process.env.CARGO_TARGET_DIR ?? path.join(v1, 'target');
const headlessBin =
  process.env.PRYSM_HEADLESS_BIN ??
  path.join(target, 'debug', 'prysm-headless');
const dist = path.join(v1, 'frontend', 'dist', 'headless', 'web-portal');
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

  constructor(command: string, args: string[]) {
    this.child = spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'] });
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

/** The entry script a page loads, e.g. `/assets/index-CZ3hLPcm.js`. */
const script = /\/assets\/index-[\w-]+\.js/;

/**
 * Fails unless the `prysm-headless` under test serves the bundle `npm run
 * build` last wrote: it embeds the bundle when cargo builds it, so a binary
 * built before the frontend serves the old one.
 */
export async function checkBundle(): Promise<void> {
  // No bundle here: the binary was built elsewhere (CI hands it to this
  // job), so there is nothing it could be stale against.
  const assets = await readFile(path.join(dist, 'assets.rs'), 'utf8').catch(
    () => null,
  );
  if (assets === null) return;
  const built = script.exec(assets)?.[0];
  const dir = await mkdtemp(path.join(tmpdir(), 'prysm-e2e-'));
  const file = path.join(dir, 'settings.json');
  await writeFile(file, JSON.stringify({ output: { kind: 'null' } }));
  const host = new Process(headlessBin, [file, '--listen', '127.0.0.1:0']);
  try {
    const line = await host.waitFor('portal http://');
    const url = /http:\/\/[^/\s]+/.exec(line)?.[0];
    if (!url) throw new Error(`no address in ${line}`);
    const served = script.exec(await (await fetch(`${url}/`)).text())?.[0];
    if (!built || served !== built)
      throw new Error(
        `${headlessBin} serves ${served ?? 'no script'}, but dist has ${built ?? 'none'}: rebuild prysm-headless (cargo build -p prysm-headless) after npm run build`,
      );
  } finally {
    await host.stop();
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
}

export interface Sender {
  /** The first output line containing `needle`: `connected`, `refused`. */
  waitFor(needle: string): Promise<string>;
}

export const test = base.extend<{
  /** The settings file's content. Override with `test.use({ settings })`. */
  settings: Settings;
  prysm: Prysm;
  /** Start a test sender streaming to this test's Prysm (stopped after the test). */
  startSender: (...args: string[]) => Sender;
}>({
  settings: [
    { capture: remoteCapture, output: { kind: 'null' } },
    { option: true },
  ],

  prysm: async ({ settings }, provide, testInfo) => {
    const dir = await mkdtemp(path.join(tmpdir(), 'prysm-e2e-'));
    const file = path.join(dir, 'settings.json');
    await writeFile(file, JSON.stringify(settings));
    const host = new Process(headlessBin, [file, '--listen', '127.0.0.1:0']);
    let code: number | null;
    try {
      const line = await host.waitFor('portal http://');
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
      });
    } finally {
      code = await host.stop();
      await testInfo.attach('prysm-headless.log', {
        body: host.lines.join('\n'),
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

export { expect };
