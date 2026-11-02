import { defineConfig } from '@playwright/test';
import base from './playwright.config';

const servers = Array.isArray(base.webServer) ? base.webServer : [];
export default defineConfig({
  ...base,
  testMatch: '**/guests.spec.ts',
  testIgnore: [],
  use: { ...base.use, baseURL: 'http://localhost:14319' },
  webServer: servers.map((server, index) =>
    index === 0
      ? {
          ...server,
          env: {
            ...server.env,
            DEEP_PUBLIC_ORIGIN: 'https://demo.friend.ts.net',
            DEEP_DATA_DIR: 'test-results/guest-data',
          },
        }
      : server,
  ),
});
