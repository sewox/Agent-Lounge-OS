/**
 * jsdom coverage: StatsStrip filter clicks, aria-pressed, active state.
 */
import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { JSDOM } from "jsdom";
import { StatsStrip, type StatsStripItem } from "./stats-strip.ts";

type HostWindow = Window & typeof globalThis;

let dom: JSDOM | null = null;
let root: Root | null = null;

const saved: {
  window?: typeof globalThis.window;
  document?: typeof globalThis.document;
  navigator?: typeof globalThis.navigator;
  IS_REACT_ACT_ENVIRONMENT?: boolean;
} = {};

function mount(activeId: string | null, onFilterChange: (id: string) => void) {
  const items: StatsStripItem[] = [
    { id: "all", label: "All", value: 10 },
    { id: "halted", label: "Halted", value: 2 },
    { id: "error", label: "Error", value: 1 },
  ];
  dom = new JSDOM('<!doctype html><html><body><div id="root"></div></body></html>', {
    url: "http://127.0.0.1/",
  });
  const win = dom.window as unknown as HostWindow;
  saved.window = globalThis.window;
  saved.document = globalThis.document;
  saved.navigator = globalThis.navigator;
  saved.IS_REACT_ACT_ENVIRONMENT = (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean })
    .IS_REACT_ACT_ENVIRONMENT;
  Object.defineProperty(globalThis, "window", { configurable: true, value: win });
  Object.defineProperty(globalThis, "document", { configurable: true, value: win.document });
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: win.navigator });
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

  const host = win.document.getElementById("root");
  assert.ok(host);
  root = createRoot(host);
  act(() => {
    root!.render(
      createElement(StatsStrip, {
        items,
        activeId,
        onFilterChange,
      }),
    );
  });
  return win.document;
}

function cleanup() {
  act(() => {
    root?.unmount();
  });
  root = null;
  dom?.window.close();
  dom = null;
  if (saved.window !== undefined) {
    Object.defineProperty(globalThis, "window", { configurable: true, value: saved.window });
  }
  if (saved.document !== undefined) {
    Object.defineProperty(globalThis, "document", { configurable: true, value: saved.document });
  }
  if (saved.navigator !== undefined) {
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: saved.navigator });
  }
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
    saved.IS_REACT_ACT_ENVIRONMENT;
}

afterEach(() => {
  cleanup();
});

describe("StatsStrip", () => {
  it("marks the active item with aria-pressed and data-active", () => {
    const doc = mount("halted", () => undefined);
    const buttons = [...doc.querySelectorAll('[data-qa="stats-strip-item"]')] as HTMLButtonElement[];
    assert.equal(buttons.length, 3);
    const halted = buttons.find((b) => b.getAttribute("data-stat-id") === "halted");
    const all = buttons.find((b) => b.getAttribute("data-stat-id") === "all");
    assert.ok(halted);
    assert.ok(all);
    assert.equal(halted.getAttribute("aria-pressed"), "true");
    assert.equal(halted.getAttribute("data-active"), "true");
    assert.equal(all.getAttribute("aria-pressed"), "false");
    // Accessible name from visible content (WCAG 2.5.3) — no overriding aria-label.
    assert.equal(halted.getAttribute("aria-label"), null);
    assert.match(halted.textContent || "", /Halted/);
    assert.match(halted.textContent || "", /2/);
    assert.equal(halted.getAttribute("title"), "Active filter");
    assert.equal(all.getAttribute("title"), "Set filter");
  });

  it("clicking a number calls onFilterChange with that category id", () => {
    const calls: string[] = [];
    const doc = mount("all", (id) => {
      calls.push(id);
    });
    const errorBtn = doc.querySelector(
      '[data-qa="stats-strip-item"][data-stat-id="error"]',
    ) as HTMLButtonElement | null;
    assert.ok(errorBtn);
    act(() => {
      errorBtn.click();
    });
    assert.deepEqual(calls, ["error"]);
  });

  it("exposes a toolbar role for keyboard discoverability", () => {
    const doc = mount(null, () => undefined);
    const strip = doc.querySelector('[data-qa="stats-strip"]');
    assert.ok(strip);
    assert.equal(strip.getAttribute("role"), "toolbar");
    assert.equal(strip.getAttribute("aria-label"), "Category filters");
  });
});
