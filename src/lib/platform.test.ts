import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  detectPlatform,
  isApplePlatform,
  paletteShortcutLabel,
  SSR_DEFAULT_PLATFORM,
} from "./platform.ts";

describe("platform helpers", () => {
  it("uses linux as SSR default (Ctrl+K)", () => {
    assert.equal(SSR_DEFAULT_PLATFORM, "linux");
    assert.equal(paletteShortcutLabel(SSR_DEFAULT_PLATFORM), "Ctrl+K");
  });

  it("maps shortcut labels per platform", () => {
    assert.equal(paletteShortcutLabel("macos"), "⌘K");
    assert.equal(paletteShortcutLabel("windows"), "Ctrl+K");
    assert.equal(paletteShortcutLabel("linux"), "Ctrl+K");
  });

  it("detectPlatform falls back to linux without navigator", () => {
    assert.equal(detectPlatform(), "linux");
  });

  it("isApplePlatform is macOS-only", () => {
    assert.equal(isApplePlatform("macos"), true);
    assert.equal(isApplePlatform("windows"), false);
    assert.equal(isApplePlatform("linux"), false);
  });
});
