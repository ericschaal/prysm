import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = new URL('openapi.json', import.meta.url);
const schema = execFileSync('cargo', [
  'run', '--locked', '--quiet', '-p', 'prysm-api', '--bin', 'export-openapi',
], {
  cwd: root,
  encoding: 'utf8',
  stdio: ['ignore', 'pipe', 'inherit'],
  maxBuffer: 16 * 1024 * 1024,
});

const current = existsSync(output) ? readFileSync(output, 'utf8') : undefined;
if (process.argv.includes('--check')) {
  if (current !== schema) throw new Error('OpenAPI schema is stale; run npm run generate-api');
} else if (current !== schema) {
  writeFileSync(output, schema);
}
