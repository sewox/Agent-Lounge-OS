import { defineConfig, devices } from "@playwright/test";
import path from "node:path";

const PORT = Number(process.env.PLAYWRIGHT_PORT || 4173);
const BASE = process.env.PLAYWRIGHT_BASE_URL || `http://127.0.0.1:${PORT}`;
/** Ignored output dir — not committed under docs/. Uploaded as CI artifact. */
const QA_OUT = path.join("test-results", "qa-baseline");

export default defineConfig({
  testDir: "./e2e/tests",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 10_000 },
  outputDir: path.join("test-results", "playwright-output"),
  reporter: [
    ["list"],
    ["json", { outputFile: path.join(QA_OUT, "playwright-report.json") }],
    ["html", { open: "never", outputFolder: "playwright-report" }],
  ],
  use: {
    baseURL: BASE,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: "off",
  },
  projects: [
    {
      name: "D0",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 },
    },
    {
      name: "D1",
      testIgnore: /hydration/,
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1512, height: 982 },
        deviceScaleFactor: 2,
      },
    },
    {
      name: "D2",
      testIgnore: /hydration/,
      use: { ...devices["Desktop Chrome"], viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 },
    },
    {
      name: "D3",
      testIgnore: /hydration/,
      use: { ...devices["Desktop Chrome"], viewport: { width: 1080, height: 1920 }, deviceScaleFactor: 1 },
    },
    {
      name: "D960",
      testMatch: /vault-dashboard|vault-projects|health-settings|shell-cross/,
      use: { ...devices["Desktop Chrome"], viewport: { width: 960, height: 800 }, deviceScaleFactor: 1 },
    },
    {
      name: "D4-scale",
      // Real 130% UI scale via storageState (al-os-ui-scale=1.3) + deviceScaleFactor.
      // a11y seal: layout-fill/no-overflow must hold at enlarged rem (root font-size > 16px).
      testMatch: /layout|vault-dashboard|health-settings/,
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1280, height: 800 },
        deviceScaleFactor: 1.3,
        storageState: path.join("e2e", "storage", "d4-scale.json"),
      },
    },
  ],
  webServer: {
    command: process.env.PLAYWRIGHT_WEB_SERVER
      || `node scripts/qa/static-server.mjs ${PORT}`,
    url: BASE,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
