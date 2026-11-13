import { JSDOM } from 'jsdom';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { createApplicationRuntime } from '@prysm/application-state';
import { fakeApplicationPort } from '../testkit/application';

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export function mountDom(html = '<div id="root"></div>') {
  const dom = new JSDOM(html);
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
 * `props` go to `Host` as well (e.g. a preview client).
 */
export async function renderPortal(Host, initial, change, props = {}) {
  const dom = mountDom();
  const fixture = fakeApplicationPort(initial);
  if (change) fixture.publish(change(fixture.snapshot()));
  const application = createApplicationRuntime(fixture.client);
  const root = createRoot(document.getElementById('root'));
  await act(async () =>
    root.render(<Host application={application} {...props} />),
  );
  return {
    dom,
    fixture,
    application,
    async unmount() {
      await act(async () => root.unmount());
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
  $$('form').find((item) => item.querySelector('h2')?.textContent === title);
export const text = () => document.body.textContent;

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

export const click = (element) => act(async () => element.click());

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
