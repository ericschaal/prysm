import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { createServer, loadConfigFromFile } from 'vite';

test('development proxy serves api.ts as JavaScript and forwards API routes', async t => {
  const root = fileURLToPath(new URL('.', import.meta.url));
  const cacheDir = await mkdtemp(join(tmpdir(), 'prysm-vite-'));
  const requests = [];
  const backend = http.createServer((request, response) => {
    requests.push(request.url);
    if (request.url !== '/api/state') {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end(JSON.stringify({ source: 'prysm' }));
  });
  let vite;
  t.after(async () => {
    await vite?.close();
    await new Promise(resolve => backend.close(resolve));
    await rm(cacheDir, { recursive: true, force: true });
  });
  await new Promise(resolve => backend.listen(0, '127.0.0.1', resolve));
  const target = `http://127.0.0.1:${backend.address().port}`;
  const { config } = await loadConfigFromFile(
    { command: 'serve', mode: 'development' }, join(root, 'vite.config.ts'),
  );
  // Keep the actual configured matchers; isolate only the backend destination.
  const proxy = Object.fromEntries(Object.entries(config.server.proxy).map(([path, options]) => [
    path, typeof options === 'string' ? target : { ...options, target },
  ]));
  vite = await createServer({
    ...config, configFile: false, root, cacheDir,
    server: { ...config.server, host: '127.0.0.1', port: 0, proxy, hmr: false },
  });
  await vite.listen();
  const origin = `http://127.0.0.1:${vite.httpServer.address().port}`;
  const module = await fetch(`${origin}/api.ts`);
  assert.equal(module.status, 200, 'api.ts must not be forwarded to Rust');
  assert.match(module.headers.get('Content-Type'), /javascript/);
  const state = await fetch(`${origin}/api/state`);
  assert.equal(state.status, 200);
  assert.deepEqual(await state.json(), { source: 'prysm' });
  assert.deepEqual(requests, ['/api/state']);
});
