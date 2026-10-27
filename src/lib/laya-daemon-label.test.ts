import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { layaDaemonLabel } from "./laya-daemon-label.ts";
import type { DecisionGateStatus } from "./lounge.ts";

const en = (key: string) => {
  const map: Record<string, string> = {
    disconnected: "Disconnected",
    running: "Running",
    checking: "Checking…",
    recommended: "recommended",
    errorShort: "Error",
  };
  return map[key] ?? key;
};

const tr = (key: string) => {
  const map: Record<string, string> = {
    disconnected: "Bağlı değil",
    running: "Çalışıyor",
    checking: "Kontrol ediliyor…",
    recommended: "önerildi",
    errorShort: "Hata",
  };
  return map[key] ?? key;
};

function status(
  partial: Partial<DecisionGateStatus> & Pick<DecisionGateStatus, "phase">,
): DecisionGateStatus {
  return {
    title: "DecisionGate",
    message: "",
    detail: null,
    device: null,
    reason: null,
    ...partial,
  };
}

describe("layaDaemonLabel", () => {
  it("ready always shows Running · device (never latency)", () => {
    assert.equal(
      layaDaemonLabel(status({ phase: "ready", device: "Metal" }), en),
      "Running · Metal",
    );
    assert.equal(
      layaDaemonLabel(status({ phase: "ready", device: "Metal" }), tr),
      "Çalışıyor · Metal",
    );
    assert.equal(layaDaemonLabel(status({ phase: "ready", device: "" }), en), "Running");
    assert.equal(layaDaemonLabel(status({ phase: "ready", device: null }), tr), "Çalışıyor");
  });

  it("localizes other phases", () => {
    assert.equal(layaDaemonLabel(null, en), "Disconnected");
    assert.equal(layaDaemonLabel(null, tr), "Bağlı değil");
    assert.equal(layaDaemonLabel(status({ phase: "loading" }), en), "Checking…");
    assert.equal(layaDaemonLabel(status({ phase: "loading" }), tr), "Kontrol ediliyor…");
    assert.equal(layaDaemonLabel(status({ phase: "available" }), en), "recommended");
    assert.equal(layaDaemonLabel(status({ phase: "available" }), tr), "önerildi");
    assert.equal(layaDaemonLabel(status({ phase: "failed" }), en), "Error");
    assert.equal(
      layaDaemonLabel(status({ phase: "failed", reason: "boom" }), en),
      "boom",
    );
  });
});
