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

    const onGha = process.env.GITHUB_ACTIONS === "true";
    const runnerOs = process.env.RUNNER_OS ?? "";
    if (onGha && runnerOs === "Linux") {
      assert.match(
        result.stdout,
        /ok: -o opts gate passes under later-sorted Retries 1 override/,
        "Linux CI must run the apt-config -o opts gate (not skip)",
      );
    } else if (runnerOs === "macOS" || runnerOs === "Windows") {
      assert.match(
        result.stdout,
        /ok: skipping apt-config override self-test \(apt-config not on this OS\)/,
        "macOS/Windows CI must emit the explicit apt-config skip line",
      );
    }
  });
});
