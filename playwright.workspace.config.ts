import { defineConfig } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import base from './playwright.config';

export default defineConfig({
  ...base,
  outputDir: 'logs/workspace-browser-results',
  use: { ...base.use, baseURL: 'http://127.0.0.1:14328' },
  webServer: [
    {
      command: 'cargo run --locked -p deep-server',
      url: 'http://127.0.0.1:14328/api/status',
      env: {
        DEEP_BIND: '127.0.0.1:14328',
        DEEP_PROVIDER: 'demo',
        DEEP_DEFAULT_BACKEND: 'demo',
        DEEP_DATA_DIR: `logs/workspace-browser-data/${randomUUID()}`,
        DEEP_WORKER_IMAGE: '',
        DEEP_IMAGE_PROFILES_FILE: '',
        DEEP_MODEL_PROFILES_FILE: '',
        DEEP_PUBLIC_ORIGIN: '',
      },
      reuseExistingServer: false,
      timeout: 180_000,
    },
    {
      command: 'npm run dev --workspace @deep/web -- --host 127.0.0.1 --port 14329',
      url: 'http://127.0.0.1:14329',
      env: { DEEP_DEV_API_URL: 'http://127.0.0.1:14328' },
      reuseExistingServer: false,
    },
  ],
});
