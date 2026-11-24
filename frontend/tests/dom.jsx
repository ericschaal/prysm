import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { ApplicationError } from '@prysm/application-client';
import { embeddedState } from '@prysm/application-client/http';
import { createApplicationRuntime } from '@prysm/application-state';
import { saveDeveloper } from '@prysm/web-portal';
import { fakeApplicationPort } from '../testkit/application';
import { fakeCaptureModesClient } from '../testkit/capture-modes';

/** Kept before a test mocks timers: navigation must still land. */
const realSetTimeout = globalThis.setTimeout;

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export function mountDom(html = '<div id="root"></div>') {
  // An address of its own, as served: the portal's hash router needs one.
  const dom = new JSDOM(html, { url: 'http://prysm.test/' });
  // jsdom draws nothing, so there is nothing to scroll.
  dom.window.scrollTo = () => {};
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    FormData: dom.window.FormData,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  return dom;
}

/**
 * Render `Host` over a fixture application; `change` edits the fixture's
 * settings before the first render (e.g. fields this UI does not show);
 * `props` go to `Host` as well (e.g. a preview client); without
 * `captureModes` every capture delivers any size and rate. `embedded`: the
 * page comes with the state, as the portal serves it; `held`: the stream
 * answers only on `answer()`, or refuses with `answer(failure)`.
 */
export async function renderPortal(
  Host,
  initial,
  change,
  props = {},
  { embedded = false, held = false } = {},
) {
  const dom = mountDom();
  const fixture = fakeApplicationPort(initial);
  if (change) fixture.publish(change(fixture.snapshot()));
  if (embedded) {
    const block = document.createElement('script');
    block.type = 'application/json';
    block.id = 'prysm-state';
    block.textContent = JSON.stringify(fixture.snapshot());
    document.head.append(block);
  }
  let answer = () => {};
  if (held) {
    const observe = fixture.client.observe;
    const answered = new Promise((resolve) => {
      answer = resolve;
    });
    fixture.client.observe = async (signal) => {
      const failure = await answered;
      if (failure) throw new ApplicationError(failure);
      return observe(signal);
    };
  }
  const application = createApplicationRuntime(
    fixture.client,
    embeddedState(document),
  );
  const root = createRoot(document.getElementById('root'));
  await act(async () =>
    root.render(
      <Host
        application={application}
        captureModes={fakeCaptureModesClient().client}
        {...props}
      />,
    ),
  );
  return {
    dom,
    fixture,
    application,
    answer: (failure) =>
      act(async () => {
        answer(failure);
        await sleep(0);
      }),
    async unmount() {
      await act(async () => root.unmount());
      // Developer options outlive a page without storage: off for the next.
      saveDeveloper(false);
      dom.window.close();
    },
  };
}

export const $ = (selector, scope = document) => scope.querySelector(selector);
export const $$ = (selector, scope = document) => [
  ...scope.querySelectorAll(selector),
];
export const field = (name) => $(`[name="${name}"]`);
export const button = (text, scope = document) =>
  $$('button', scope).find((item) => item.textContent.trim() === text);
export const form = (title) =>
  $$('form').find(
    (item) =>
      item.querySelector('h2')?.textContent === title ||
      item.getAttribute('aria-label') === title,
  );
export const text = () => document.body.textContent;

/*
 * A failed `assert.equal(node, other)` prints both values in full: for a DOM
 * node that walks the whole document and its React fibers, which takes
 * gigabytes. Assertions about elements pass `assert` a boolean and name the
 * elements with `nameOf`.
 */

/** A short name for an element in an assertion message: `<button> Keep`. */
function nameOf(element) {
  if (!element) return String(element);
  const tag = element.tagName.toLowerCase();
  const id = element.id ? `#${element.id}` : '';
  const name = element.getAttribute('name');
  const named = name ? `[name="${name}"]` : '';
  const words = element.textContent.trim().slice(0, 40);
  return `<${tag}${id}${named}>${words ? ` ${words}` : ''}`;
}

/** Assert that `actual` is the element `expected`, naming both on failure. */
export function assertSame(actual, expected, message = 'same element') {
  if (actual !== expected)
    assert.fail(`${message}: ${nameOf(actual)} is not ${nameOf(expected)}`);
}

/** Assert that `element` has focus, naming what has it on failure. */
export const assertFocused = (element, message = 'focus') =>
  assertSame(document.activeElement, element, message);

/** Type into an input or choose an option the way a user would. */
export async function edit(element, value) {
  const view = element.ownerDocument.defaultView;
  const prototype =
    element.tagName === 'SELECT'
      ? view.HTMLSelectElement.prototype
      : view.HTMLInputElement.prototype;
  await act(async () => {
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(
      element,
      String(value),
    );
    element.dispatchEvent(
      new view.Event(element.tagName === 'SELECT' ? 'change' : 'input', {
        bubbles: true,
      }),
    );
  });
}

/**
 * The tasks a navigation takes to land: going back is a history traversal,
 * which jsdom (like a browser) queues and performs in tasks of their own
 * after the click, before the router renders the page it hears of. Opening
 * a page needs fewer.
 */
const navigationTasks = 3;

/** Let a navigation land. */
export async function navigated() {
  for (let task = 0; task < navigationTasks; task += 1)
    await new Promise((resolve) => realSetTimeout(resolve, 0));
}

/** Let a navigation land while a test mocks `timers` (jsdom runs on them). */
export async function navigatedOn(timers) {
  for (let task = 0; task < navigationTasks; task += 1)
    await act(async () => timers.tick(0));
}

export const click = (element) =>
  act(async () => {
    element.click();
    await navigated();
  });

/** Open Settings from Home, the way a user would. */
export const openSettings = () =>
  act(async () => {
    $('.header-link').click();
    await navigated();
  });

/** A row on Settings by its label: `Video source`, `Calibrations`. */
export const row = (label) =>
  $$('.row').find((item) => $('.row-label', item)?.textContent === label);

/** A section's kind rows (`capture.kind`), while it offers a choice. */
export const kindRows = (path) => $(`[data-field="${path}"]`);
/** The kinds offered, in order: `['device', 'remote']`. */
export const kinds = (path) =>
  $$('input[type="radio"]', kindRows(path)).map((input) => input.value);
/** The ticked kind. */
export const kindOf = (path) =>
  $$('input[type="radio"]', kindRows(path)).find((input) => input.checked)
    ?.value;
/** Tick a kind, as a user does. */
export const chooseKind = (path, kind) =>
  click($(`input[value="${kind}"]`, kindRows(path)));

/** A choice's radio by its row's label: `Balanced`, `Custom`, `Dark`. */
export const radio = (label) => $('input[type="radio"]', row(label));

/** A row's value line: its facts, or its live problem in their place. */
export const valueOf = (label) => $('.row-value', row(label)).textContent;

/** Open a page from Settings by its row: `Calibrations`, `Style`. */
export async function openPage(label) {
  if (!row(label)) await openSettings();
  await act(async () => {
    $('.row-link', row(label)).click();
    await navigated();
  });
}

/** From Home or Settings: the editor of one part of the setup. */
export const openEditor = openPage;

/** Leave an editor, dropping what it holds, for Settings. */
export const leaveEditor = () =>
  act(async () => {
    $('.page-heading button').click();
    await navigated();
  });

export async function submit(element) {
  const view = element.ownerDocument.defaultView;
  await act(async () =>
    element.dispatchEvent(
      new view.Event('submit', { bubbles: true, cancelable: true }),
    ),
  );
  // Let the command resolve and the stream deliver the new state.
  await act(async () => sleep(0));
}

/** The error message shown for a control, through its `aria-describedby`. */
export function errorOf(element) {
  return (element.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id))
    .find((node) => node?.classList.contains('field-error'))?.textContent;
}
