import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/browser',
  testIgnore: '**/guests.spec.ts',
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  use: {
    baseURL: 'http://127.0.0.1:14318',
    headless: true,
    launchOptions: { executablePath: process.env.DEEP_BROWSER_PATH || undefined },
  },
  webServer: [
    {
      command: 'cargo run --locked -p deep-server',
      url: 'http://127.0.0.1:14318/api/status',
      env: {
        DEEP_BIND: '127.0.0.1:14318',
        DEEP_PROVIDER: 'demo',
        DEEP_DEFAULT_BACKEND: 'demo',
        DEEP_DATA_DIR: 'test-results/browser-data',
        DEEP_WORKER_IMAGE: '',
        DEEP_IMAGE_PROFILES_FILE: '',
        DEEP_MODEL_PROFILES_FILE: '',
        DEEP_PUBLIC_ORIGIN: '',
      },
      reuseExistingServer: false,
      timeout: 180_000,
    },
    {
      command: 'npm run dev --workspace @deep/web -- --host 127.0.0.1 --port 14319',
      url: 'http://127.0.0.1:14319',
      env: { DEEP_DEV_API_URL: 'http://127.0.0.1:14318' },
      reuseExistingServer: false,
    },
  ],
});
