// E2E ngang chức năng (PLAN P5) chạy trên hub demo của agent-hub (FakeRuntime).
// WebKit của Playwright = đại diện gần nhất cho WKWebView của app trên macOS (cùng engine, KHÔNG phải app thật).
// E2E trên app thật (tauri-driver + WebDriver) chỉ chạy được trên Windows/Linux — xem tests/e2e/tauri-driver/ và CI.
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.ts",
  globalSetup: "./global-setup.ts",
  globalTeardown: "./global-teardown.ts",
  // One shared demo hub with seeded state (one approval, …): run in order, one worker.
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  timeout: 60_000,
  reporter: process.env.CI ? [["list"], ["html", { open: "never", outputFolder: "../../playwright-report" }]] : "list",
  outputDir: "../../test-results",
  use: {
    trace: "retain-on-failure",
    acceptDownloads: true,
    locale: "vi-VN",
  },
  projects: [{ name: "webkit", use: { ...devices["Desktop Safari"] } }],
});
