// The root eslint.config.mjs rule set for the v1 workspace, which installs its
// own packages (the root config ignores v1/**). Merge the two at consolidation.
import js from '@eslint/js';
import { defineConfig } from 'eslint/config';
import tseslint from 'typescript-eslint';
import react from '@eslint-react/eslint-plugin';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import jsxA11y from 'eslint-plugin-jsx-a11y-x';
import prettier from 'eslint-config-prettier/flat';
import globals from 'globals';

export default defineConfig(
  {
    ignores: [
      '**/node_modules/**',
      'dist/**',
      '.test/**',
      'packages/contracts/generated/**',
    ],
  },
  { linterOptions: { reportUnusedDisableDirectives: 'error' } },
  js.configs.recommended,
  {
    files: ['**/*.{js,mjs,cjs}'],
    languageOptions: { globals: globals.node },
  },
  {
    files: ['**/*.{ts,tsx,mts,cts}'],
    extends: [tseslint.configs.recommendedTypeChecked],
    rules: { 'no-nested-ternary': 'error' },
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
      globals: globals.browser,
    },
  },
  {
    files: ['**/*.{jsx,tsx}'],
    extends: [
      react.configs.recommended,
      jsxA11y.configs.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: { globals: globals.browser },
  },
  {
    files: ['**/*.tsx'],
    extends: [react.configs['recommended-type-checked']],
  },
  {
    // The root applies these to its React app (crates/web); here that is every package.
    files: ['**/*.{ts,tsx,jsx}'],
    extends: [reactHooks.configs.flat.recommended],
  },
  {
    // DOM tests run under node with jsdom globals.
    files: ['tests/**/*.{js,jsx,mjs}'],
    languageOptions: { globals: { ...globals.node, ...globals.browser } },
  },
  prettier,
);
