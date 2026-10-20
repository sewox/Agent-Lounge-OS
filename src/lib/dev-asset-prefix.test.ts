import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import {
  resolveDevAssetPrefix,
  resolveDevPort,
} from "./dev-asset-prefix.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

describe("dev assetPrefix / PORT", () => {
  it("defaults to 3000 when PORT is unset or empty", () => {
    assert.equal(resolveDevPort({}), "3000");
    assert.equal(resolveDevPort({ PORT: "" }), "3000");
  });

  it("follows PORT for the non-prod assetPrefix", () => {
    assert.equal(
      resolveDevAssetPrefix({
        isProd: false,
        host: "localhost",
        port: resolveDevPort({ PORT: "4123" }),
      }),
      "http://localhost:4123",
    );
    assert.equal(
      resolveDevAssetPrefix({
        isProd: false,
        host: "127.0.0.1",
        port: resolveDevPort({ PORT: "3001" }),
      }),
      "http://127.0.0.1:3001",
    );
  });

  it("omits assetPrefix in production", () => {
    assert.equal(
      resolveDevAssetPrefix({ isProd: true, port: "4123" }),
      undefined,
    );
  });

  it("next.config.ts wires resolveDevAssetPrefix to PORT", () => {
    const conf = fs.readFileSync(path.join(root, "next.config.ts"), "utf8");
    assert.match(conf, /resolveDevAssetPrefix/);
    assert.match(conf, /resolveDevPort/);
    assert.match(conf, /from\s+["'].*dev-asset-prefix["']/);
  });

  it("scripts/dev-port.mjs matches resolveDevPort for the same PORT", () => {
    const probe = `
      import { resolveDevPort } from ${JSON.stringify(
        path.join(root, "scripts/dev-port.mjs"),
      )};
      const port = resolveDevPort({ PORT: process.env.PROBE_PORT });
      process.stdout.write(port);
    `;
    const result = spawnSync(process.execPath, ["--input-type=module", "-e", probe], {
      encoding: "utf8",
      env: { ...process.env, PROBE_PORT: "4455" },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "4455");
    assert.equal(resolveDevPort({ PORT: "4455" }), "4455");
  });

  it("package.json dev script pins next via next-dev.mjs", () => {
    const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
    assert.equal(pkg.scripts.dev, "node scripts/next-dev.mjs");
    const nextDev = fs.readFileSync(path.join(root, "scripts/next-dev.mjs"), "utf8");
    assert.match(nextDev, /resolveDevPort/);
    assert.match(nextDev, /["']dev["']\s*,\s*["']-p["']/);
  });
});
