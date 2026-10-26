import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    // Preserve the browser Host so the controller can validate its Origin.
    proxy: { '/api/': { target: 'http://127.0.0.1:8787', changeOrigin: false } },
  },
  build: { target: 'es2020' },
});
