import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

/* The build output is embedded into the Tauri binary and served by the gateway
   (src-tauri/build.rs walks dist/ and generates an asset table), so:
   - relative asset paths, because the page is served from / by two different
     hosts (tauri://localhost and http://<mac>.local:8787);
   - no code splitting: one JS file keeps the embedded table trivial and the
     first paint on a phone over WiFi a single request. */
export default defineConfig({
  plugins: [react()],
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsInlineLimit: 4096,
    rollupOptions: {
      output: {
        manualChunks: undefined,
        entryFileNames: 'app.[hash].js',
        chunkFileNames: 'app.[hash].js',
        assetFileNames: 'app.[hash][extname]',
      },
    },
  },
  server: { port: 5173, strictPort: false },
});
