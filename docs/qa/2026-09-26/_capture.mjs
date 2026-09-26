/**
 * QA screenshot capture — audit tooling only (not shipped product).
 * Captures every route/modal at 1280x800 and 1536x960 with mock + empty data.
 */
import { chromium } from "playwright";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const BASE = process.env.QA_BASE_URL || "http://localhost:3000";
const OUT = path.resolve("docs/qa/2026-09-26");
const VIEWPORTS = [
  { name: "1280x800", width: 1280, height: 800 },
  { name: "1536x960", width: 1536, height: 960 },
];

const MOCK_ROUTES = [
  { id: "onboarding", path: "/onboarding" },
  { id: "dashboard", path: "/dashboard" },
  { id: "stream", path: "/stream" },
  { id: "vault", path: "/vault" },
  { id: "health", path: "/health" },
  { id: "fleet", path: "/fleet" },
  { id: "telemetry", path: "/telemetry" },
  { id: "quotas", path: "/quotas" },
  { id: "settings", path: "/settings" },
  { id: "dashboard-routing-banner", path: "/dashboard?demo=routing-banner" },
];

const EMPTY_ROUTES = MOCK_ROUTES.map((r) => {
  if (r.path.includes("?")) {
    return { id: r.id, path: r.path.replace("demo=routing-banner", "demo=empty") };
  }
  return { id: r.id, path: `${r.path}?demo=empty` };
}).filter((r) => r.id !== "dashboard-routing-banner");

async function shot(page, name, viewport) {
  const file = `${name}__${viewport.name}.png`;
  const dest = path.join(OUT, file);
  await page.screenshot({ path: dest, fullPage: false });
  console.log("wrote", file);
  return file;
}

async function openPalette(page) {
  await page.keyboard.press("Control+k");
  await page.waitForTimeout(500);
}

async function captureSuite(browser, label, routes) {
  const manifest = [];
  for (const vp of VIEWPORTS) {
    const context = await browser.newContext({
      viewport: { width: vp.width, height: vp.height },
      deviceScaleFactor: 1,
    });
    const page = await context.newPage();
    for (const route of routes) {
      await page.goto(`${BASE}${route.path}`, { waitUntil: "networkidle", timeout: 90000 });
      await page.waitForTimeout(800);
      const file = await shot(page, `${label}__${route.id}`, vp);
      manifest.push({ label, route: route.id, viewport: vp.name, file, path: route.path });

      if (route.id === "onboarding") {
        continue;
      }

      if (route.id === "dashboard" || route.id === "vault") {
        await openPalette(page);
        const palFile = await shot(page, `${label}__${route.id}-palette`, vp);
        manifest.push({
          label,
          route: `${route.id}-palette`,
          viewport: vp.name,
          file: palFile,
          path: route.path,
        });
        await page.keyboard.press("Escape");
        await page.waitForTimeout(250);

        // Try select first map node (mock data names)
        const candidates = page.getByText(/Agent-Lounge-OS|EchoMind|codebase-memory/i);
        if ((await candidates.count()) > 0) {
          await candidates.first().click({ timeout: 3000 }).catch(() => {});
          await page.waitForTimeout(500);
          const selFile = await shot(page, `${label}__${route.id}-selected`, vp);
          manifest.push({
            label,
            route: `${route.id}-selected`,
            viewport: vp.name,
            file: selFile,
            path: route.path,
          });
        }
      }
    }
    await context.close();
  }
  return manifest;
}

async function main() {
  await mkdir(OUT, { recursive: true });
  const browser = await chromium.launch({
    headless: true,
    args: ["--font-render-hinting=none", "--disable-font-subpixel-positioning"],
  });

  console.log("capturing mock…");
  const mock = await captureSuite(browser, "mock", MOCK_ROUTES);
  console.log("capturing empty…");
  const empty = await captureSuite(browser, "empty", EMPTY_ROUTES);

  await writeFile(
    path.join(OUT, "manifest.json"),
    JSON.stringify({ base: BASE, capturedAt: new Date().toISOString(), mock, empty }, null, 2),
  );
  await browser.close();
  console.log("done", mock.length + empty.length, "shots");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
