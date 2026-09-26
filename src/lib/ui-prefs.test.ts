import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { ToolQuota } from "./lounge.ts";
import {
  DEFAULT_UI_SCALE,
  isPanelCollapsed,
  nextUiScale,
  parseUiScale,
  quotaCriticality,
  readPanelCollapsed,
  readStoredUiScale,
  selectCriticalQuotas,
  writePanelCollapsed,
  writeStoredUiScale,
} from "./ui-prefs.ts";

function quota(partial: Partial<ToolQuota> & Pick<ToolQuota, "id" | "tool">): ToolQuota {
  return {
    kind: "api",
    unit: "req",
    used: "0",
    remaining: "100",
    reset: "daily",
    percent: 0,
    tone: "ok",
    label: "ok",
    ...partial,
  };
}

describe("selectCriticalQuotas", () => {
  it("returns the 3 most critical by usage and alert state", () => {
    const rows = [
      quota({ id: "a", tool: "Alpha", percent: 40, tone: "ok" }),
      quota({ id: "b", tool: "Beta", percent: 92, tone: "warn" }),
      quota({ id: "c", tool: "Gamma", percent: 100, tone: "amber", exhausted: true }),
      quota({ id: "d", tool: "Delta", percent: 85, tone: "warn" }),
      quota({ id: "e", tool: "Echo", percent: 10, tone: "ok" }),
    ];
    const top = selectCriticalQuotas(rows, 3);
    assert.deepEqual(
      top.map((row) => row.id),
      ["c", "b", "d"],
    );
  });

  it("ranks amber/exhausted above raw percent", () => {
    const lowAmber = quota({ id: "amber", tool: "Amber", percent: 50, tone: "amber" });
    const highOk = quota({ id: "ok", tool: "Ok", percent: 95, tone: "ok" });
    assert.ok(quotaCriticality(lowAmber) > quotaCriticality(highOk));
  });
});

describe("ui scale persistence", () => {
  it("parses known scales and falls back to 100%", () => {
    assert.equal(parseUiScale("1.15"), 1.15);
    assert.equal(parseUiScale("2"), DEFAULT_UI_SCALE);
    assert.equal(parseUiScale(null), DEFAULT_UI_SCALE);
  });

  it("steps up/down within bounds", () => {
    assert.equal(nextUiScale(1, 1), 1.15);
    assert.equal(nextUiScale(1.3, 1), 1.3);
    assert.equal(nextUiScale(0.9, -1), 0.9);
  });

  it("round-trips through a storage mock", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => {
        store.set(key, value);
      },
    };
    writeStoredUiScale(1.3, storage);
    assert.equal(readStoredUiScale(storage), 1.3);
  });
});

describe("panel collapse persistence", () => {
  it("round-trips collapsed map", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => {
        store.set(key, value);
      },
    };
    writePanelCollapsed({ "event-stream": true, vault: false }, storage);
    const map = readPanelCollapsed(storage);
    assert.equal(isPanelCollapsed("event-stream", map), true);
    assert.equal(isPanelCollapsed("vault", map), false);
    assert.equal(isPanelCollapsed("missing", map, false), false);
  });
});
