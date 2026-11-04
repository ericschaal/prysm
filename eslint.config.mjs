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
      '**/dist/**',
      '**/target/**',
      '**/*.generated.*',
      'work/**',
      '.webos-sdk/**',
    ],
  },
  { linterOptions: { reportUnusedDisableDirectives: 'error' } },
  js.configs.recommended,
  {
    files: ['**/*.{js,mjs,cjs}'],
    languageOptions: { globals: globals.node },
  },
  {
    files: ['packaging/webos/tests/*.js'],
    languageOptions: { sourceType: 'commonjs' },
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
    files: ['**/vite.config.ts'],
    languageOptions: { globals: globals.node },
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
    files: ['crates/web/**/*.{ts,tsx,js,jsx}'],
    extends: [reactHooks.configs.flat.recommended],
  },
  prettier,
);
