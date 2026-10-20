/**
 * jsdom coverage for synchronous alertdialog focus trap.
 * Proves Tab immediately after open is not undone by a delayed boot focus.
 */
import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";
import { act, createElement, useRef, useState, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { JSDOM } from "jsdom";
import {
  handleAlertdialogTabKey,
  listFocusable,
  resolveInitialFocus,
  useAlertdialogFocusTrap,
} from "./use-alertdialog-focus-trap.ts";

type HostWindow = Window & typeof globalThis;

let dom: JSDOM | null = null;
let root: Root | null = null;
const saved: {
  window?: typeof globalThis.window;
  document?: typeof globalThis.document;
  navigator?: typeof globalThis.navigator;
  IS_REACT_ACT_ENVIRONMENT?: boolean;
} = {};

function installDom() {
  uninstallDom();
  dom = new JSDOM("<!DOCTYPE html><html><body><div id='root'></div></body></html>", {
    url: "http://127.0.0.1/",
    pretendToBeVisual: true,
  });
  const win = dom.window as unknown as HostWindow;
  saved.window = globalThis.window;
  saved.document = globalThis.document;
  saved.navigator = globalThis.navigator;
  saved.IS_REACT_ACT_ENVIRONMENT = (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean })
    .IS_REACT_ACT_ENVIRONMENT;
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: win,
  });
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    writable: true,
    value: win.document,
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    writable: true,
    value: win.navigator,
  });
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
}

function uninstallDom() {
  if (root) {
    act(() => {
      root!.unmount();
    });
    root = null;
  }
  if (dom) {
    dom.window.close();
    dom = null;
  }
  if (saved.window !== undefined) {
    Object.defineProperty(globalThis, "window", { configurable: true, value: saved.window });
  }
  if (saved.document !== undefined) {
    Object.defineProperty(globalThis, "document", { configurable: true, value: saved.document });
  }
  if (saved.navigator !== undefined) {
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      value: saved.navigator,
    });
  }
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
    saved.IS_REACT_ACT_ENVIRONMENT;
}

afterEach(() => {
  uninstallDom();
});

function HaltFixture({ startOpen = false }: { startOpen?: boolean }): ReactElement {
  const [open, setOpen] = useState(startOpen);
  const containerRef = useRef<HTMLDivElement | null>(null);
  useAlertdialogFocusTrap({
    open,
    containerRef,
    initialFocusSelector: '[data-qa="halt-reason-input"]',
    onEscape: () => setOpen(false),
  });
  return createElement(
    "div",
    null,
    createElement(
      "button",
      {
        type: "button",
        "data-qa": "halt-toggle",
        onClick: () => setOpen(true),
      },
      "Halt",
    ),
    open
      ? createElement(
          "div",
          {
            ref: containerRef,
            role: "alertdialog",
            "data-qa": "halt-confirm-dialog",
          },
          createElement("input", { "data-qa": "halt-reason-input" }),
          createElement("button", { type: "button", "data-qa": "halt-dialog-cancel" }, "Cancel"),
          createElement("button", { type: "button", "data-qa": "halt-dialog-confirm" }, "Confirm"),
        )
      : null,
  );
}

function qa(sel: string): HTMLElement {
  const el = document.querySelector(sel);
  assert.ok(el, sel);
  return el as HTMLElement;
}

describe("alertdialog focus helpers", () => {
  it("resolveInitialFocus prefers selector then first focusable", () => {
    installDom();
    const rootEl = document.createElement("div");
    rootEl.innerHTML =
      '<input data-qa="halt-reason-input" /><button data-qa="halt-dialog-cancel">C</button>';
    document.body.appendChild(rootEl);
    assert.equal(
      resolveInitialFocus(rootEl, '[data-qa="halt-reason-input"]')?.getAttribute("data-qa"),
      "halt-reason-input",
    );
    assert.equal(listFocusable(rootEl).length, 2);
  });

  it("handleAlertdialogTabKey wraps last → first", () => {
    installDom();
    const rootEl = document.createElement("div");
    rootEl.innerHTML =
      '<input data-qa="halt-reason-input" /><button data-qa="halt-dialog-cancel">C</button><button data-qa="halt-dialog-confirm">OK</button>';
    document.body.appendChild(rootEl);
    const confirm = qa('[data-qa="halt-dialog-confirm"]');
    confirm.focus();
    let prevented = false;
    handleAlertdialogTabKey(
      {
        key: "Tab",
        shiftKey: false,
        preventDefault: () => {
          prevented = true;
        },
      },
      rootEl,
      confirm,
    );
    assert.equal(prevented, true);
    assert.equal(document.activeElement?.getAttribute("data-qa"), "halt-reason-input");
  });
});

describe("useAlertdialogFocusTrap", () => {
  it("focuses reason input synchronously on open (useLayoutEffect, no setTimeout)", () => {
    installDom();
    const mount = document.getElementById("root");
    assert.ok(mount);
    root = createRoot(mount);
    act(() => {
      root!.render(createElement(HaltFixture, { startOpen: false }));
    });
    const trigger = qa('[data-qa="halt-toggle"]');
    trigger.focus();
    act(() => {
      trigger.click();
    });
    assert.equal(
      document.activeElement?.getAttribute("data-qa"),
      "halt-reason-input",
      "initial focus must land before paint / any macrotask",
    );
  });

  it("Tab immediately after open: wrap from Confirm is not stolen by a late boot", async () => {
    installDom();
    const mount = document.getElementById("root");
    assert.ok(mount);
    root = createRoot(mount);
    act(() => {
      root!.render(createElement(HaltFixture, { startOpen: false }));
    });

    act(() => {
      qa('[data-qa="halt-toggle"]').click();
    });
    assert.equal(document.activeElement?.getAttribute("data-qa"), "halt-reason-input");

    // User/e2e Tabs to Confirm immediately (jsdom does not move focus on Tab).
    const confirm = qa('[data-qa="halt-dialog-confirm"]');
    confirm.focus();
    assert.equal(document.activeElement, confirm);

    // Immediate wrap Tab (what CTL-05 does at the last control).
    const tab = new window.KeyboardEvent("keydown", {
      key: "Tab",
      bubbles: true,
      cancelable: true,
    });
    window.dispatchEvent(tab);
    assert.equal(
      document.activeElement?.getAttribute("data-qa"),
      "halt-reason-input",
      "trap must wrap on the first Tab after open",
    );

    // Move to Confirm again, then flush a macrotask — old setTimeout(0) boot
    // would yank focus back to the reason input here.
    confirm.focus();
    await act(async () => {
      await new Promise<void>((resolve) => {
        setTimeout(resolve, 0);
      });
    });
    assert.equal(
      document.activeElement?.getAttribute("data-qa"),
      "halt-dialog-confirm",
      "no delayed boot may re-steal focus after the user already Tabbed",
    );
  });

  it("restores focus to the trigger on close", () => {
    installDom();
    const mount = document.getElementById("root");
    assert.ok(mount);
    root = createRoot(mount);
    act(() => {
      root!.render(createElement(HaltFixture, { startOpen: false }));
    });
    const trigger = qa('[data-qa="halt-toggle"]');
    trigger.focus();
    act(() => {
      trigger.click();
    });
    act(() => {
      window.dispatchEvent(
        new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
      );
    });
    assert.equal(document.activeElement?.getAttribute("data-qa"), "halt-toggle");
  });
});
