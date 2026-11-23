// One bundle per DOM test file in `.test`, so `node --test` runs each file in
// a process of its own. Code the files share goes to chunks next to them, at
// the same depth, so `import.meta.url` resolves as it does from `tests`.
import { readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

const tests = fileURLToPath(new URL('.', import.meta.url));
const input = Object.fromEntries(
  readdirSync(tests)
    .filter((name) => name.endsWith('.test.jsx'))
    .map((name) => [name.replace(/\.jsx$/, ''), `${tests}${name}`]),
);

export default defineConfig({
  build: {
    ssr: true,
    outDir: '.test',
    emptyOutDir: true,
    rolldownOptions: {
      input,
      output: {
        entryFileNames: '[name].js',
        chunkFileNames: '[name]-[hash].js',
      },
    },
  },
});
