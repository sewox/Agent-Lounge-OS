import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  ALLOWED_DEEP_LINK_OPEN_ROUTES,
  isAllowedDeepLinkOpenRoute,
} from "./lounge.ts";

/** Must stay in lockstep with Rust `ALLOWED_OPEN_ROUTES` in services/deep_link.rs. */
const RUST_ALLOWED_OPEN_ROUTES = [
  "/",
  "/dashboard",
  "/stream",
  "/vault",
  "/health",
  "/fleet",
  "/telemetry",
  "/quotas",
  "/settings",
  "/onboarding",
] as const;

describe("deep-link open route allowlist", () => {
  it("mirrors the 10 Rust ALLOWED_OPEN_ROUTES exactly", () => {
    assert.equal(ALLOWED_DEEP_LINK_OPEN_ROUTES.length, 10);
    assert.deepEqual([...ALLOWED_DEEP_LINK_OPEN_ROUTES], [...RUST_ALLOWED_OPEN_ROUTES]);
  });

  it("accepts only exact allowlisted routes", () => {
    for (const route of RUST_ALLOWED_OPEN_ROUTES) {
      assert.equal(isAllowedDeepLinkOpenRoute(route), true, route);
    }
    assert.equal(isAllowedDeepLinkOpenRoute("/health?tab=agents"), false);
    assert.equal(isAllowedDeepLinkOpenRoute("//host"), false);
    assert.equal(isAllowedDeepLinkOpenRoute("/dashboard/extra"), false);
    assert.equal(isAllowedDeepLinkOpenRoute("javascript:alert(1)"), false);
  });
});
