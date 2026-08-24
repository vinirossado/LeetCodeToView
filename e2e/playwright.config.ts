import { defineConfig, devices } from '@playwright/test';

// Runs against the docker-compose stack (localhost:4200), NOT `ng serve`.
// This matters: `environment.development.ts` (used by `ng serve`) hardcodes
// an absolute `ws://localhost:8080` for wsBaseUrl, which would NOT reproduce
// the real WebSocket-URL bug found and fixed this session (relative-URL
// construction only breaks when wsBaseUrl is '', i.e. the production/
// same-origin build served by nginx in docker-compose.yml). These tests
// exist specifically to catch that class of bug, so they must exercise the
// production build behind the real nginx proxy.
export default defineConfig({
  testDir: './tests',
  fullyParallel: false,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: 'http://localhost:4200',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
