import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  computeTokenRowHeight,
  DEAD_SYMBOL_ROW_HEIGHT_REM,
  PAGE_ROW_HEIGHT_REM,
  readRootFontSizePx,
} from "./row-height.ts";

describe("computeTokenRowHeight", () => {
  it("matches the legacy 36px page row at 1.0 scale (16px root)", () => {
    assert.equal(computeTokenRowHeight(16, PAGE_ROW_HEIGHT_REM), 36);
  });

  it("scales page rows for 1.25 UI scale", () => {
    // 16 * 1.25 = 20 → 20 * 2.25 = 45
    assert.equal(computeTokenRowHeight(20, PAGE_ROW_HEIGHT_REM), 45);
  });

  it("scales page rows for 1.5 UI scale", () => {
    // 16 * 1.5 = 24 → 24 * 2.25 = 54
    assert.equal(computeTokenRowHeight(24, PAGE_ROW_HEIGHT_REM), 54);
  });

  it("scales dead-symbol rows for 1.0 / 1.25 / 1.5", () => {
    assert.equal(computeTokenRowHeight(16, DEAD_SYMBOL_ROW_HEIGHT_REM), 52);
    assert.equal(computeTokenRowHeight(20, DEAD_SYMBOL_ROW_HEIGHT_REM), 65);
    assert.equal(computeTokenRowHeight(24, DEAD_SYMBOL_ROW_HEIGHT_REM), 78);
  });

  it("guards non-positive font size and token", () => {
    assert.equal(computeTokenRowHeight(0), 16 * PAGE_ROW_HEIGHT_REM);
    assert.equal(computeTokenRowHeight(-4), 16 * PAGE_ROW_HEIGHT_REM);
    assert.equal(computeTokenRowHeight(16, 0), 16 * PAGE_ROW_HEIGHT_REM);
  });
});

describe("readRootFontSizePx", () => {
  it("returns 16 when document is unavailable", () => {
    assert.equal(readRootFontSizePx(null), 16);
    assert.equal(readRootFontSizePx(undefined), 16);
  });
});
