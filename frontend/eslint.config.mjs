// Lint rules for the frontend workspace.
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
      '.e2e/**',
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
    // Served to the browser as it is.
    files: ['packages/web-portal/public/**/*.js'],
    languageOptions: { globals: globals.browser, sourceType: 'script' },
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
    // Hooks live in .ts modules too, so the hook rules cover every source file.
    files: ['**/*.{ts,tsx,jsx}'],
    extends: [reactHooks.configs.flat.recommended],
  },
  {
    files: [
      'packages/application-client/http.ts',
      'packages/webos-platform/index.ts',
    ],
    rules: {
      'no-restricted-syntax': [
        'error',
        {
          selector: 'TSAsExpression',
          message:
            'Validate external data against its generated contract before reading typed fields.',
        },
      ],
    },
  },
  {
    // DOM tests run under node with jsdom globals.
    files: ['tests/**/*.{js,jsx,mjs}'],
    languageOptions: { globals: { ...globals.node, ...globals.browser } },
  },
  {
    // A failed assert prints its values in full, and a DOM node in full is
    // the whole document with its React fibers: gigabytes.
    files: ['tests/**/*.test.{js,jsx,mjs,tsx}'],
    rules: {
      'no-restricted-syntax': [
        'error',
        {
          selector:
            'CallExpression:matches([callee.name="assert"], [callee.object.name="assert"]) > MemberExpression[object.name="document"][property.name="activeElement"]',
          message:
            'Use assertFocused(element) from tests/dom.jsx: a failed assert prints the focused node in full.',
        },
        {
          selector:
            'CallExpression:matches([callee.name="assert"], [callee.object.name="assert"]) BinaryExpression > MemberExpression[object.name="document"][property.name="activeElement"]',
          message:
            'Use assertFocused(element) from tests/dom.jsx instead of comparing with document.activeElement.',
        },
      ],
    },
  },
  {
    // Playwright specs and fixtures run under node.
    files: ['e2e/**/*.ts'],
    languageOptions: { globals: globals.node },
  },
  prettier,
);
