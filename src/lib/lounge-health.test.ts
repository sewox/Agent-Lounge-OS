import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  AVAIL_NOT_INSTALLED,
  CODE_RESTART_EXHAUSTED,
  CODE_RESTART_RETRYING,
  coreServicesDegraded,
  degradedCoreServiceNames,
  isNotInstalled,
  optionalMissingServiceNames,
  resolveDegradedRestart,
  type ServiceHealth,
  type ServiceReport,
} from "./lounge.ts";

function health(
  id: string,
  name: string,
  running: boolean,
  extra: Partial<ServiceHealth> = {},
): ServiceHealth {
  return {
    id,
    name,
    running,
    started_by_us: false,
    endpoint: "http://127.0.0.1:0",
    detail: null,
    error: running ? null : "down",
    ...extra,
  };
}

function report(partial: Partial<ServiceReport>): ServiceReport {
  return {
    ollama: health("ollama", "LMR", true),
    nats: health("nats", "NATS", true),
    memory: health("memory", "Memory", true),
    plugin: health("plugin", "Plugin", true),
    ...partial,
  };
}

describe("lounge health helpers", () => {
  it("treats not_installed LMR as optional, not degraded", () => {
    const next = report({
      ollama: health("ollama", "LMR", false, {
        availability: AVAIL_NOT_INSTALLED,
        error: null,
        detail: "optional — place binary in data/lmr",
      }),
    });
    assert.equal(isNotInstalled(next.ollama), true);
    assert.equal(coreServicesDegraded(next), false);
    assert.deepEqual(degradedCoreServiceNames(next), []);
    assert.deepEqual(optionalMissingServiceNames(next), ["LMR"]);
    assert.equal(resolveDegradedRestart(next), null);
  });

  it("marks crashed LMR as degraded", () => {
    const next = report({
      ollama: health("ollama", "LMR", false, {
        error: "Service Degraded — auto-restart limiti aşıldı (5 deneme)",
      }),
    });
    assert.equal(coreServicesDegraded(next), true);
    assert.deepEqual(degradedCoreServiceNames(next), ["LMR"]);
    const phase = resolveDegradedRestart(next);
    assert.ok(phase);
    assert.equal(phase?.phase.kind, "exhausted");
  });

  it("resolves restart phase from machine code, not Turkish error text", () => {
    const next = report({
      ollama: health("ollama", "LMR", false, {
        code: CODE_RESTART_EXHAUSTED,
        detail: "max=5",
        error: "opaque localized failure text without limit markers",
      }),
    });
    const phase = resolveDegradedRestart(next);
    assert.equal(phase?.phase.kind, "exhausted");
    if (phase?.phase.kind === "exhausted") {
      assert.equal(phase.phase.max, 5);
    }

    const retrying = report({
      nats: health("nats", "NATS", false, {
        code: CODE_RESTART_RETRYING,
        detail: "attempt=2 max=5 wait=4",
        error: "opaque retry text",
      }),
    });
    const retryPhase = resolveDegradedRestart(retrying);
    assert.equal(retryPhase?.phase.kind, "retrying");
    if (retryPhase?.phase.kind === "retrying") {
      assert.equal(retryPhase.phase.attempt, 2);
      assert.equal(retryPhase.phase.max, 5);
      assert.equal(retryPhase.phase.waitSecs, 4);
    }
  });
});
