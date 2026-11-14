// Licence notices for the third-party JavaScript that the build really bundles:
// the packages whose modules end up in the output chunks, read from
// node_modules (offline). The TV packages ship the result.
import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';

const NODE_MODULES = '/node_modules/';

/** The package directory a bundled module comes from, or undefined for Prysm's own code. */
export function packageDir(moduleId) {
  const id = moduleId.replace(/^\0/, '').replace(/\?.*$/, '');
  const at = id.lastIndexOf(NODE_MODULES);
  if (at < 0) return undefined;
  const start = at + NODE_MODULES.length;
  const parts = id.slice(start).split('/');
  const name = parts.slice(0, parts[0].startsWith('@') ? 2 : 1).join('/');
  return id.slice(0, start) + name;
}

/** A build plugin that adds each bundled package's directory to `packages`. */
export function bundledPackages(packages) {
  return {
    name: 'prysm-bundled-packages',
    generateBundle(_, bundle) {
      for (const output of Object.values(bundle)) {
        for (const id of output.moduleIds ?? []) {
          const dir = packageDir(id);
          if (dir) packages.add(dir);
        }
      }
    },
  };
}

/** One text file with each package's name, version, licence and licence text. */
export async function notices(packageDirs) {
  const entries = await Promise.all(
    [...new Set(packageDirs)].map(async (dir) => {
      const { name, version, license } = JSON.parse(
        await readFile(join(dir, 'package.json'), 'utf8'),
      );
      const file = (await readdir(dir)).find((entry) =>
        /^licen[cs]e(\.(md|txt))?$/i.test(entry),
      );
      if (!file) throw new Error(`${name} ${version} has no licence file`);
      const text = (await readFile(join(dir, file), 'utf8')).trim();
      return { name, text: `${name} ${version} (${license})\n\n${text}\n` };
    }),
  );
  entries.sort((a, b) => (a.name < b.name ? -1 : 1));
  const rule = '-'.repeat(78);
  return [
    'Third-party JavaScript bundled in the Prysm app and web portal.\n',
    ...entries.map((entry) => entry.text),
  ].join(`\n${rule}\n\n`);
}
