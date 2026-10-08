import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { AMBER_THRESHOLD, type ToolQuota } from "./lounge.ts";
import {
  amberRowsSignature,
  clampBadgeIndex,
  quotaBadgeName,
  quotaBadgePercent,
  quotaBadgeTone,
  QUOTA_BADGE_ERROR_THRESHOLD,
  selectAmberQuotaRows,
} from "./quota-badge.ts";

function quota(partial: Partial<ToolQuota> & Pick<ToolQuota, "id" | "tool">): ToolQuota {
  return {
    kind: "api",
    unit: "req",
    used: "0",
    remaining: "100",
    reset: "daily",
    percent: 0,
    tone: "ok",
    label: "",
    ...partial,
  };
}

describe("selectAmberQuotaRows", () => {
  it("filters by AMBER_THRESHOLD and sorts highest percent first", () => {
    const rows = [
      quota({ id: "a", tool: "Alpha", percent: 81 }),
      quota({ id: "b", tool: "Beta", percent: 98 }),
      quota({ id: "c", tool: "Gamma", percent: 40 }),
      quota({ id: "d", tool: "Delta", percent: 85 }),
    ];
    assert.deepEqual(
      selectAmberQuotaRows(rows).map((row) => row.id),
      ["b", "d", "a"],
    );
  });

  it("excludes boundary below threshold and includes at/above", () => {
    const rows = [
      quota({ id: "low", tool: "Low", percent: 79.9 }),
      quota({ id: "edge", tool: "Edge", percent: AMBER_THRESHOLD }),
      quota({ id: "high", tool: "High", percent: 80.1 }),
    ];
    assert.deepEqual(
      selectAmberQuotaRows(rows).map((row) => row.id),
      ["high", "edge"],
    );
  });

  it("treats null percent as 0 (excluded)", () => {
    const rows = [
      quota({ id: "n", tool: "Null", percent: null }),
      quota({ id: "ok", tool: "Ok", percent: 90 }),
    ];
    assert.deepEqual(
      selectAmberQuotaRows(rows).map((row) => row.id),
      ["ok"],
    );
  });

  it("stable tie-breaks equal percent by label then id", () => {
    const rows = [
      quota({ id: "z", tool: "Zeta", label: "b-label", percent: 90 }),
      quota({ id: "a", tool: "Alpha", label: "a-label", percent: 90 }),
      quota({ id: "m", tool: "Mu", label: "a-label", percent: 90 }),
    ];
    assert.deepEqual(
      selectAmberQuotaRows(rows).map((row) => row.id),
      ["a", "m", "z"],
    );
  });

  it("keeps exhausted rows when percent is at threshold", () => {
    const rows = [
      quota({
        id: "ex",
        tool: "Exhausted",
        percent: 100,
        exhausted: true,
        tone: "amber",
      }),
      quota({ id: "w", tool: "Warn", percent: 82 }),
    ];
    const amber = selectAmberQuotaRows(rows);
    assert.equal(amber[0]?.id, "ex");
    assert.equal(quotaBadgeTone(amber[0]!), "error");
  });
});

describe("quotaBadgeName / tone / helpers", () => {
  it("prefers tool, then label, then id", () => {
    assert.equal(quotaBadgeName(quota({ id: "x", tool: "Claude", label: "near" })), "Claude");
    assert.equal(quotaBadgeName(quota({ id: "x", tool: "", label: "Fallback" })), "Fallback");
    assert.equal(quotaBadgeName(quota({ id: "raw-id", tool: "", label: "" })), "raw-id");
  });

  it("maps tone by error threshold and exhausted", () => {
    assert.equal(
      quotaBadgeTone(quota({ id: "a", tool: "A", percent: AMBER_THRESHOLD })),
      "amber",
    );
    assert.equal(
      quotaBadgeTone(quota({ id: "b", tool: "B", percent: QUOTA_BADGE_ERROR_THRESHOLD })),
      "error",
    );
    assert.equal(
      quotaBadgeTone(quota({ id: "c", tool: "C", percent: 88, exhausted: true })),
      "error",
    );
    assert.equal(quotaBadgeTone(quota({ id: "d", tool: "D", percent: 10 })), "ok");
  });

  it("rounds percent and clamps index", () => {
    assert.equal(quotaBadgePercent(quota({ id: "a", tool: "A", percent: 98.6 })), 99);
    assert.equal(quotaBadgePercent(quota({ id: "b", tool: "B", percent: null, exhausted: true })), 100);
    assert.equal(clampBadgeIndex(2, 2), 0);
    assert.equal(clampBadgeIndex(1, 3), 1);
    assert.equal(clampBadgeIndex(0, 0), 0);
  });

  it("builds a stable signature for rotation reset", () => {
    const rows = [
      quota({ id: "a", tool: "A", percent: 90 }),
      quota({ id: "b", tool: "B", percent: 85 }),
    ];
    assert.equal(amberRowsSignature(rows), "a:90|b:85");
  });
});
