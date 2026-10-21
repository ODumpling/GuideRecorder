import { defineConfig, devices } from "@playwright/test";

// Editor flows against the Vite dev server (not Tauri), with the IPC mocked
// by e2e/mock-entry.ts and served from the fixture guides by e2e/backend.ts.
const PORT = 1431;

export default defineConfig({
  testDir: "e2e",
  outputDir: "e2e/test-results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  timeout: 15_000,
  expect: { timeout: 5_000 },
  reporter: "list",
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: "retain-on-failure",
  },
  // The specs walk the macOS app; Desktop Chrome's user agent is Windows,
  // which src/lib/platform.ts would take for the Windows app.
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"], userAgent: devices["Desktop Safari"].userAgent },
    },
  ],
  webServer: {
    command: `pnpm exec vite --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}/e2e/index.html`,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
