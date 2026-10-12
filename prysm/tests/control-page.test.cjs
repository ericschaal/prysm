// Run with: node prysm/tests/control-page.test.cjs (no frontend dependencies).
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const html = fs.readFileSync(path.join(__dirname, '../src/control.html'), 'utf8');
const elements = new Map();
const element = id => {
  if (!elements.has(id)) elements.set(id, { disabled: false, hidden: false, setAttribute() {} });
  return elements.get(id);
};
const buttons = ['prysm', 'wled'].map(source => ({ dataset: { source }, setAttribute() {} }));
const offline = { source: 'prysm', observed: null, pending: false, operation_id: 0,
  observation_error: 'WLED offline', error: null, wled_url: 'http://192.168.18.39/' };
const requests = [];
const context = vm.createContext({
  document: { hidden: false, getElementById: element, querySelectorAll: () => buttons, addEventListener() {} },
  sessionStorage: { getItem: () => '', setItem() {}, removeItem() {} },
  AbortController, setTimeout: () => 0, clearTimeout() {}, setInterval() {},
  fetch: async (_, options) => {
    requests.push(options);
    return { ok: true, status: 200, json: async () => options.method === 'PUT' ? { operation_id: 1 } : offline };
  },
});
vm.runInContext(html.match(/<script>([\s\S]*?)<\/script>/)[1], context);
context.render(offline);
assert.equal(element('off').disabled, false, 'Off must work when WLED observation fails');
assert.equal(element('power').disabled, true, 'Unknown power cannot safely toggle');
assert.match(element('status').textContent, /Unknown/);
const observed = { power: true, realtime: true, live_override: 0, service: 'active', substate: 'running' };
context.render({ ...offline, observed, source: 'wled', observation_error: null });
assert.match(element('status').textContent, /not confirmed/);
assert.equal(element('retry').hidden, false);
context.render({ ...offline, observed: { ...observed, power: false }, observation_error: null });
assert.match(element('status').textContent, /not confirmed/, 'Power=false with active DDP is not confirmed dark');
assert.equal(element('power').textContent, 'Lights: Unknown');
context.render({ ...offline, observed, pending: true });
assert.equal(element('off').disabled, true);
context.render(offline);
vm.runInContext("token = 'a'.repeat(32)", context);
(async () => {
  await element('off').onclick();
  assert.deepEqual(JSON.parse(requests[0].body), { power: false, source: 'prysm' });
  assert.equal(element('off').disabled, false);
  console.log('Control page state checks passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
