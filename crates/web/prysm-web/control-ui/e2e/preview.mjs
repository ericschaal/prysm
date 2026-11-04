import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve, extname, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { calibrationFixture } from './fixture.mjs';
const root = fileURLToPath(new URL('../dist/', import.meta.url));
let fixture = calibrationFixture();
const types = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.css': 'text/css',
};
createServer(async (request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  response.setHeader('Cache-Control', 'no-store');
  try {
    if (path === '/__reset' && request.method === 'POST') {
      fixture = calibrationFixture();
      response.writeHead(303, { Location: '/#/calibrate/local-demo' }).end();
      return;
    }
    if (path.startsWith('/api/')) {
      let body = '';
      for await (const chunk of request) body += chunk;
      const result = fixture.respond(
        path,
        request.method,
        body ? JSON.parse(body) : {},
      );
      response
        .writeHead(result.status ?? 200, { 'Content-Type': 'application/json' })
        .end(JSON.stringify(result.json));
      return;
    }
    const file = resolve(root, `.${path === '/' ? '/index.html' : path}`);
    if (!file.startsWith(root.endsWith(sep) ? root : root + sep)) {
      response.writeHead(404).end();
      return;
    }
    let contents = await readFile(file);
    if (extname(file) === '.html')
      contents = contents
        .toString()
        .replace(
          '<body>',
          '<body><aside style="padding:8px 16px;background:#ece8f1;color:#594967;text-align:center;font:12px system-ui">Local simulation · No TV or lights connected <form action="/__reset" method="post" style="display:inline"><button style="min-height:32px;margin-left:12px;padding:4px 10px;font-size:12px">Restart demo</button></form></aside>',
        );
    response
      .writeHead(200, {
        'Content-Type': types[extname(file)] ?? 'application/octet-stream',
      })
      .end(contents);
  } catch {
    response.writeHead(404).end('Not found');
  }
}).listen(5180, '127.0.0.1', () =>
  process.stdout.write(
    'Calibration simulation: http://127.0.0.1:5180/#/calibrate/local-demo\n',
  ),
);
