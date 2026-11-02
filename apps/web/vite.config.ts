import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, '../..', 'DEEP_');
  return {
    plugins: [react()],
    resolve: {
      dedupe: ['dompurify'],
      alias: [
        {
          // Monaco 0.57 also embeds DOMPurify; use the patched package for that copy.
          find: './dompurify/dompurify.js',
          replacement: fileURLToPath(new URL('./src/editor-sanitizer.ts', import.meta.url)),
        },
      ],
    },
    server: {
      port: 5173,
      strictPort: true,
      proxy: {
        '/api': { target: env.DEEP_DEV_API_URL || `http://${env.DEEP_BIND || '127.0.0.1:4318'}` },
      },
    },
  };
});
