import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { describe, it } from "node:test";
import path from "node:path";

const script = path.resolve("scripts/ci/install-linux-build-deps.sh");

describe("install-linux-build-deps", () => {
  it("passes --self-test (retry, ::error::, package contract)", () => {
    const result = spawnSync("bash", [script, "--self-test"], {
      encoding: "utf8",
      env: process.env,
    });
    assert.equal(
      result.status,
      0,
      `self-test failed:\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
    assert.match(result.stdout, /install-linux-build-deps self-test: ok/);
  });
});
