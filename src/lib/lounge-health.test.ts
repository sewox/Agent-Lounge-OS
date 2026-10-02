import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  AVAIL_NOT_INSTALLED,
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
});
