import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  modeShowsDetailInline,
  modeShowsInspectorInline,
  PANEL_MEDIUM_MAX,
  PANEL_NARROW_MAX,
  resolvePanelMode,
} from "./panel-mode.ts";

describe("resolvePanelMode", () => {
  it("returns one below narrow breakpoint (landscape)", () => {
    assert.equal(resolvePanelMode(1199, 900), "one");
    assert.equal(resolvePanelMode(800, 600), "one");
  });

  it("returns two in the medium band", () => {
    assert.equal(resolvePanelMode(PANEL_NARROW_MAX, 900), "two");
    assert.equal(resolvePanelMode(1500, 900), "two");
    assert.equal(resolvePanelMode(PANEL_MEDIUM_MAX - 1, 1000), "two");
  });

  it("returns three at and above wide", () => {
    assert.equal(resolvePanelMode(PANEL_MEDIUM_MAX, 1000), "three");
    assert.equal(resolvePanelMode(1920, 1080), "three");
  });

  it("portrait aspect stacks regardless of width", () => {
    assert.equal(resolvePanelMode(1080, 1920), "portrait-stack");
    assert.equal(resolvePanelMode(900, 1600), "portrait-stack");
    // Even a "wide" CSS width is portrait when height is larger.
    assert.equal(resolvePanelMode(1800, 2000), "portrait-stack");
  });

  it("guards non-positive boxes as one", () => {
    assert.equal(resolvePanelMode(0, 0), "one");
    assert.equal(resolvePanelMode(-1, 800), "one");
  });
});

describe("modeShows* helpers", () => {
  it("detail inline for two/three/portrait", () => {
    assert.equal(modeShowsDetailInline("one"), false);
    assert.equal(modeShowsDetailInline("two"), true);
    assert.equal(modeShowsDetailInline("three"), true);
    assert.equal(modeShowsDetailInline("portrait-stack"), true);
  });

  it("inspector inline only for three", () => {
    assert.equal(modeShowsInspectorInline("one"), false);
    assert.equal(modeShowsInspectorInline("two"), false);
    assert.equal(modeShowsInspectorInline("three"), true);
    assert.equal(modeShowsInspectorInline("portrait-stack"), false);
  });
});
