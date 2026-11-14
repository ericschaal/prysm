// React DOM decides at load time whether the `input` event exists, so a DOM
// must be global before it is imported; each test then mounts its own.
import { JSDOM } from 'jsdom';

const { window } = new JSDOM('');
Object.assign(globalThis, { window, document: window.document });
