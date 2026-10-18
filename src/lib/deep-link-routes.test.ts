import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import {
  ALLOWED_DEEP_LINK_OPEN_ROUTES,
  isAllowedDeepLinkOpenRoute,
} from "./lounge.ts";

const here = path.dirname(fileURLToPath(import.meta.url));
const deepLinkRs = path.resolve(here, "../../src-tauri/src/services/deep_link.rs");

/** Parse `ALLOWED_OPEN_ROUTES` string literals from Rust source (no hardcoded mirror). */
function parseRustAllowedOpenRoutes(source: string): string[] {
  const start = source.indexOf("pub const ALLOWED_OPEN_ROUTES");
  assert.ok(start >= 0, "ALLOWED_OPEN_ROUTES not found in deep_link.rs");
  const slice = source.slice(start);
  const bracket = slice.indexOf("[");
  const end = slice.indexOf("];", bracket);
  assert.ok(bracket >= 0 && end > bracket, "ALLOWED_OPEN_ROUTES array bounds");
  const body = slice.slice(bracket, end + 1);
  const routes = [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1]!);
  assert.ok(routes.length > 0, "parsed zero routes from ALLOWED_OPEN_ROUTES");
  return routes;
}

describe("deep-link open route allowlist", () => {
  it("mirrors the Rust ALLOWED_OPEN_ROUTES from deep_link.rs exactly", () => {
    const rustRoutes = parseRustAllowedOpenRoutes(fs.readFileSync(deepLinkRs, "utf8"));
    assert.equal(ALLOWED_DEEP_LINK_OPEN_ROUTES.length, rustRoutes.length);
    assert.equal(rustRoutes.length, 10);
    assert.deepEqual([...ALLOWED_DEEP_LINK_OPEN_ROUTES], rustRoutes);
  });

  it("accepts only exact allowlisted routes", () => {
    for (const route of ALLOWED_DEEP_LINK_OPEN_ROUTES) {
      assert.equal(isAllowedDeepLinkOpenRoute(route), true, route);
    }
    assert.equal(isAllowedDeepLinkOpenRoute("/health?tab=agents"), false);
    assert.equal(isAllowedDeepLinkOpenRoute("//host"), false);
    assert.equal(isAllowedDeepLinkOpenRoute("/dashboard/extra"), false);
    assert.equal(isAllowedDeepLinkOpenRoute("javascript:alert(1)"), false);
  });
});
