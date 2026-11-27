/**
 * Helpers for the TV app's end-to-end tests: open the in-memory demo, press
 * remote keys, and check what the screen shows after each one. Arrows and OK
 * are real key presses; Back is keyCode 461 as the webOS remote sends it.
 *
 * Every `press` checks the focus (one focused control, visible, inside the
 * viewport and the safe area tv.css keeps, with a visible indicator), so each
 * journey checks it after every key.
 */
import { expect, type Page } from '@playwright/test';
import type { LightingTarget } from '@prysm/contracts/application';
import type { State } from '@prysm/contracts/luna';

export type Flavor = State['flavor'];
export type Key =
  'ArrowUp' | 'ArrowDown' | 'ArrowLeft' | 'ArrowRight' | 'Enter' | 'Back';
export const arrows = [
  'ArrowUp',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
] as const;

/** Select screen sync from the initial Off state of the WLED demo. */
export async function startScreenSync(page: Page) {
  await press(page, 'Enter');
  await expect(page.locator('#light-power')).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await pressUntil(page, 'ArrowRight', 'mode-screen_sync');
  await press(page, 'Enter');
  await expect(page.locator('#status-title')).toHaveText('Screen sync on');
}

/** The keys pressed on each page, named in failure messages. */
const pressed = new WeakMap<Page, string[]>();
const history = (page: Page) => {
  const keys = pressed.get(page) ?? [];
  pressed.set(page, keys);
  return keys;
};
const after = (page: Page) =>
  `after ${history(page).join(' ') || 'opening the page'}`;

/** The demo page of one package with its query flags (testkit/tv-demo.tsx). */
export function demoUrl(flavor: Flavor, flags: string[] = []) {
  const dir = flavor === 'full' ? 'webos' : 'webos-slim';
  const query = flags.length ? `?${flags.join('&')}` : '';
  return `/${dir}/tv-ui/demo.html${query}`;
}

/**
 * Open the demo of one package with its query flags and wait until focus
 * rests on `focus` (an id): the state the user starts from.
 */
export async function openTv(
  page: Page,
  flavor: Flavor,
  { flags = [], focus }: { flags?: string[]; focus: string },
) {
  await page.goto(demoUrl(flavor, flags));
  pressed.set(page, []);
  await expect(page.locator(`#${focus}`)).toBeFocused();
  await settled(page);
  await expectFocus(page);
}

/** Until no finite animation or transition runs (focus lift, dialog arrival). */
export async function settled(page: Page) {
  await page.waitForFunction(() =>
    document
      .getAnimations()
      .every(
        (animation) =>
          animation.playState !== 'running' ||
          animation.effect?.getComputedTiming().iterations === Infinity,
      ),
  );
}

/** Press one remote key, wait for the screen to settle, then check focus. */
export async function press(
  page: Page,
  key: Key,
  { check = true }: { check?: boolean } = {},
) {
  history(page).push(key);
  if (key === 'Back')
    await page.evaluate(() => {
      // webOS sends Back as keyCode 461 without a standard `key`.
      const event = new KeyboardEvent('keydown', {
        key: 'Unidentified',
        bubbles: true,
        cancelable: true,
      });
      Object.defineProperty(event, 'keyCode', { value: 461 });
      Object.defineProperty(event, 'which', { value: 461 });
      (document.activeElement ?? document.body).dispatchEvent(event);
    });
  else await page.keyboard.press(key);
  await settled(page);
  if (check) await expectFocus(page);
}

/** Press `key` until `id` has focus, at most `limit` times. */
export async function pressUntil(page: Page, key: Key, id: string, limit = 8) {
  for (let i = 0; i < limit; i++) {
    if ((await focusedId(page)) === id) return;
    await press(page, key);
  }
  expect(await focusedId(page), `${key} reaches #${id} ${after(page)}`).toBe(
    id,
  );
}

/** The focused element's id (or a description of it). */
export function focusedId(page: Page): Promise<string> {
  return page.evaluate(() => {
    const active = document.activeElement;
    if (!active || active === document.body) return '(nothing)';
    return active.id || `${active.tagName.toLowerCase()}.${active.className}`;
  });
}

/**
 * One control has focus; it is visible (on top at its centre), inside the
 * viewport as drawn, inside tv.css's safe area (`--safe-x`, `--safe-y`) as
 * laid out (the focus lift may scale it a little beyond, by design), and it
 * has a visible focus indicator: a ring (outline or box shadow) that it does
 * not have without focus.
 */
export async function expectFocus(page: Page) {
  const problems = await page.evaluate(() => {
    const out: string[] = [];
    const active = document.activeElement as HTMLElement | null;
    if (!active || active === document.body) return ['nothing has focus'];
    const name = active.id ? `#${active.id}` : active.tagName;
    const focused = document.querySelectorAll(':focus').length;
    if (focused !== 1) out.push(`${focused} elements match :focus`);
    if (
      !active.checkVisibility({
        opacityProperty: true,
        visibilityProperty: true,
      })
    )
      out.push(`${name} is not visible`);
    const rect = active.getBoundingClientRect();
    const [width, height] = [innerWidth, innerHeight];
    if (
      rect.left < 0 ||
      rect.top < 0 ||
      rect.right > width ||
      rect.bottom > height
    )
      out.push(
        `${name} is drawn outside the viewport: ${JSON.stringify(rect)}`,
      );
    const hit = document.elementFromPoint(
      rect.left + rect.width / 2,
      rect.top + rect.height / 2,
    );
    if (!hit || !active.contains(hit))
      out.push(
        `${name} is covered at its centre by ${hit ? hit.id || hit.className || hit.tagName : 'nothing'}`,
      );
    // The layout box: the element's own scale undone around its origin.
    const style = getComputedStyle(active);
    const matrix = new DOMMatrixReadOnly(
      style.transform === 'none' ? undefined : style.transform,
    );
    const [ox = 0, oy = 0] = style.transformOrigin
      .split(' ')
      .map((value) => parseFloat(value));
    const left = rect.left - ox * (1 - matrix.a);
    const top = rect.top - oy * (1 - matrix.d);
    const right = left + rect.width / matrix.a;
    const bottom = top + rect.height / matrix.d;
    const probe = document.createElement('div');
    probe.style.cssText =
      'position:fixed;width:var(--safe-x);height:var(--safe-y);visibility:hidden';
    document.body.append(probe);
    const safe = probe.getBoundingClientRect();
    probe.remove();
    const slack = 0.5;
    if (
      left < safe.width - slack ||
      top < safe.height - slack ||
      right > width - safe.width + slack ||
      bottom > height - safe.height + slack
    )
      out.push(
        `${name} lies outside the safe area (${safe.width} × ${safe.height} px): ${JSON.stringify({ left, top, right, bottom })}`,
      );
    // The indicator: compare with the same element without focus.
    const ring = (s: CSSStyleDeclaration) => ({
      outline:
        s.outlineStyle === 'none' || parseFloat(s.outlineWidth) === 0
          ? 'none'
          : `${s.outlineStyle} ${s.outlineWidth} ${s.outlineColor}`,
      shadow: s.boxShadow,
      border: s.borderColor,
      fill: s.backgroundColor,
    });
    const withFocus = ring(getComputedStyle(active));
    active.blur();
    const without = ring(getComputedStyle(active));
    active.focus({ preventScroll: true });
    const hasRing = withFocus.outline !== 'none' || withFocus.shadow !== 'none';
    const differs =
      withFocus.outline !== without.outline ||
      withFocus.shadow !== without.shadow;
    if (!hasRing || !differs)
      out.push(
        `${name} has no focus ring of its own: ${JSON.stringify({ withFocus, without })}`,
      );
    return out;
  });
  await settled(page);
  expect(problems, `focus ${after(page)}`).toEqual([]);
}

/** The enabled, visible controls of the current screen or open dialog. */
export function controls(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const scope =
      document.getElementById('modal') ??
      document.getElementById('app') ??
      document.querySelector('.calibration');
    const selector =
      'button, input, select, textarea, a[href], [tabindex]:not([tabindex="-1"])';
    return Array.from(scope?.querySelectorAll<HTMLElement>(selector) ?? [])
      .filter(
        (element) =>
          !(element as HTMLButtonElement).disabled &&
          element.checkVisibility({ visibilityProperty: true }),
      )
      .map(
        (element) =>
          element.id || `${element.tagName.toLowerCase()}.${element.className}`,
      )
      .sort();
  });
}

/**
 * Every control reachable from the focused one with arrows: a breadth-first
 * walk of the focus graph. Each control found is focused again (by script)
 * to try the four arrows from it; returns the controls reached, sorted, and
 * leaves focus where it started.
 */
export async function walk(page: Page): Promise<string[]> {
  const start = await focusedId(page);
  const seen = new Set([start]);
  const queue = [start];
  const refocus = async (id: string) => {
    await page.evaluate((each) => document.getElementById(each)?.focus(), id);
    await settled(page);
  };
  for (let id = queue.shift(); id !== undefined; id = queue.shift()) {
    for (const key of arrows) {
      await refocus(id);
      await press(page, key);
      const reached = await focusedId(page);
      if (!seen.has(reached)) {
        seen.add(reached);
        queue.push(reached);
      }
    }
  }
  await refocus(start);
  return [...seen].sort();
}

/** Arrows reach every enabled control on this screen or dialog. */
export async function expectAllReachable(page: Page) {
  const all = await controls(page);
  expect(all.length, 'the screen has controls').toBeGreaterThan(0);
  expect(await walk(page), `reachable with arrows ${after(page)}`).toEqual(all);
}

/**
 * Sizes for a 10-foot screen, as tv.css states them: every visible control
 * at least `--hit` (78 px at 1080p; LG asks for 75 px) in its smaller
 * dimension, as laid out; every visible text at least 20 px (LG's floor).
 */
export async function expectSizes(page: Page) {
  const problems = await page.evaluate(() => {
    const out: string[] = [];
    const probe = document.createElement('div');
    probe.style.cssText = 'position:fixed;height:var(--hit);visibility:hidden';
    document.body.append(probe);
    const hit = probe.getBoundingClientRect().height;
    probe.remove();
    if (!(hit >= 75)) out.push(`--hit is ${hit} px`);
    const tv = document.getElementById('root');
    for (const control of Array.from(
      tv?.querySelectorAll<HTMLElement>('button, input') ?? [],
    )) {
      if (!control.checkVisibility({ visibilityProperty: true })) continue;
      const smaller = Math.min(control.offsetWidth, control.offsetHeight);
      if (smaller < hit)
        out.push(
          `#${control.id || control.className} is ${control.offsetWidth} × ${control.offsetHeight} px`,
        );
    }
    const texts = document.createTreeWalker(
      tv ?? document.body,
      NodeFilter.SHOW_TEXT,
    );
    const range = document.createRange();
    for (let node = texts.nextNode(); node; node = texts.nextNode()) {
      const parent = node.parentElement;
      if (!parent || !node.textContent?.trim()) continue;
      if (
        !parent.checkVisibility({
          opacityProperty: true,
          visibilityProperty: true,
        })
      )
        continue;
      range.selectNodeContents(node);
      const box = range.getBoundingClientRect();
      // Read out, not shown (clipped to 1 px).
      if (box.width <= 1 || box.height <= 1) continue;
      const size = parseFloat(getComputedStyle(parent).fontSize);
      if (size < 20)
        out.push(`"${node.textContent.trim().slice(0, 40)}" is ${size} px`);
    }
    return out;
  });
  expect(problems, `sizes ${after(page)}`).toEqual([]);
}

/** Where an element is laid out, in page pixels. */
export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * The layout box of the first element matching each selector, or `null` for
 * none. Offsets, not drawn rectangles: transforms (the focus lift, the
 * screen's arrival) do not move the layout.
 */
export function layoutBoxes(
  page: Page,
  selectors: string[],
): Promise<Record<string, Box | null>> {
  return page.evaluate((all) => {
    const boxes: Record<string, Box | null> = {};
    for (const selector of all) {
      const element = document.querySelector<HTMLElement>(selector);
      if (!element) {
        boxes[selector] = null;
        continue;
      }
      let [left, top] = [0, 0];
      for (
        let at: HTMLElement | null = element;
        at;
        at = at.offsetParent as HTMLElement | null
      ) {
        left += at.offsetLeft;
        top += at.offsetTop;
      }
      boxes[selector] = {
        left,
        top,
        width: element.offsetWidth,
        height: element.offsetHeight,
      };
    }
    return boxes;
  }, selectors);
}

/** `window.demo`: the demo's in-memory supervisor and portal (testkit/tv-demo.tsx). */
interface Demo {
  supervisor: {
    /** The published state. */
    state(): State;
    /** Facts of the supervisor's own package only (testkit/luna.ts). */
    publish(change: Record<string, unknown>): void;
    /** Hold every call until `release()`. */
    hold(): void;
    release(): void;
  };
  application: {
    snapshot(): { target: LightingTarget };
    publish(change: Record<string, unknown>): void;
  };
  addLights(): void;
}
declare global {
  interface Window {
    demo: Demo;
  }
}

/** What the supervisor holds: its package's intent, the saved preferences. */
export const supervisorState = (page: Page) =>
  page.evaluate(() => window.demo.supervisor.state());
/** What the lights do, as the portal says. */
export const lightingTarget = (page: Page) =>
  page.evaluate(() => window.demo.application.snapshot().target);
/** What the portal's setup wizard does when it saves. */
export const addLights = (page: Page) =>
  page.evaluate(() => window.demo.addLights());
/** The supervisor publishes a log of `lines` messages. */
export const publishLog = (page: Page, lines: number) =>
  page.evaluate(
    (count) =>
      window.demo.supervisor.publish({
        log: Array.from(
          { length: count },
          (_, i) =>
            `21:${String(i).padStart(2, '0')}:04 worker: capture restarted (attempt ${i + 1})`,
        ),
      }),
    lines,
  );
