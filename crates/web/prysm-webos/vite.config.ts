import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  base: './',
  server: { host: '127.0.0.1', port: 5174, strictPort: true },
  plugins: [
    react(),
    {
      name: 'webos-classic-script',
      apply: 'build',
      transformIndexHtml: {
        order: 'post',
        handler: (html) => html.replace(/type="module" crossorigin/g, 'defer'),
      },
    },
  ],
  build: {
    // webOS 4.x uses Chromium 53, which cannot load ES modules.
    target: 'chrome53',
    modulePreload: false,
    rolldownOptions: { output: { format: 'iife' } },
  },
});
