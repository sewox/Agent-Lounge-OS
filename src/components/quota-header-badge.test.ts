/**
 * jsdom + fake-timer coverage for the rotating header quota badge.
 */
import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it, mock } from "node:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { act, createElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import i18n from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { JSDOM } from "jsdom";
import type { ToolQuota } from "../lib/lounge.ts";
import { QUOTA_BADGE_ROTATE_MS } from "../lib/quota-badge.ts";
import { QuotaHeaderBadge } from "./quota-header-badge.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const LOCALES = join(HERE, "../lib/i18n/locales");

function loadShell(locale: "en" | "tr"): Record<string, string> {
  return JSON.parse(readFileSync(join(LOCALES, locale, "shell.json"), "utf8")) as Record<
    string,
    string
  >;
}

type HostWindow = Window & typeof globalThis;

let dom: JSDOM | null = null;
let root: Root | null = null;
let reducedMotion = false;
let i18nReady = false;

const saved: {
  window?: typeof globalThis.window;
  document?: typeof globalThis.document;
  navigator?: typeof globalThis.navigator;
  IS_REACT_ACT_ENVIRONMENT?: boolean;
} = {};

function quota(partial: Partial<ToolQuota> & Pick<ToolQuota, "id" | "tool">): ToolQuota {
  return {
    kind: "subscription",
    unit: "subscription",
    used: "0",
    remaining: "100",
    reset: "Mon",
    percent: 0,
    tone: "ok",
    label: "status",
    ...partial,
  };
}

function StubLink(props: {
  href: string;
  children?: ReactNode;
  [key: string]: unknown;
}) {
  const { children, href, ...rest } = props;
  return createElement("a", { href, ...rest }, children);
}

async function ensureI18n(locale: "en" | "tr") {
  if (!i18nReady) {
    await i18n.use(initReactI18next).init({
      resources: {
        en: { shell: loadShell("en") },
        tr: { shell: loadShell("tr") },
      },
      lng: locale,
      fallbackLng: "en",
      supportedLngs: ["en", "tr"],
      ns: ["shell"],
      defaultNS: "shell",
      interpolation: { escapeValue: false },
      react: { useSuspense: false },
    });
    i18nReady = true;
  } else {
    await i18n.changeLanguage(locale);
  }
}

function installDom() {
  uninstallDom();
  dom = new JSDOM("<!DOCTYPE html><html><body><div id='root'></div></body></html>", {
    url: "http://127.0.0.1/",
    pretendToBeVisual: true,
  });
  const win = dom.window as unknown as HostWindow;

  win.matchMedia = ((query: string) => {
    const matches =
      reducedMotion && String(query).includes("prefers-reduced-motion: reduce");
    return {
      matches,
      media: query,
      onchange: null,
      addListener: () => undefined,
      removeListener: () => undefined,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    };
  }) as typeof win.matchMedia;

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
    try {
      root.unmount();
    } catch {
      /* already unmounted */
    }
    root = null;
  }
  if (dom) {
    dom.window.close();
    dom = null;
  }
  if ("window" in saved) {
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      writable: true,
      value: saved.window,
    });
  }
  if ("document" in saved) {
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      writable: true,
      value: saved.document,
    });
  }
  if ("navigator" in saved) {
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      writable: true,
      value: saved.navigator,
    });
  }
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
    saved.IS_REACT_ACT_ENVIRONMENT;
}

async function renderBadge(quotas: ToolQuota[], locale: "en" | "tr" = "en") {
  await ensureI18n(locale);
  const rootEl = document.getElementById("root");
  assert.ok(rootEl);
  if (!root) {
    root = createRoot(rootEl);
  }
  await act(async () => {
    root!.render(
      createElement(
        I18nextProvider,
        { i18n },
        createElement(QuotaHeaderBadge, {
          quotas,
          LinkComponent: StubLink,
        }),
      ),
    );
  });
  // Flush matchMedia / reduced-motion effect.
  await act(async () => {
    await Promise.resolve();
  });
  return rootEl;
}

function badgeEl(): Element {
  const el = document.querySelector("[data-testid='quota-header-badge']");
  assert.ok(el);
  return el;
}

function visibleLabel(el: Element): string {
  const clone = el.cloneNode(true) as Element;
  clone.querySelector("[aria-live='polite']")?.remove();
  return (clone.textContent || "").replace(/\s+/g, " ").trim();
}

describe("QuotaHeaderBadge", () => {
  beforeEach(() => {
    reducedMotion = false;
    mock.timers.enable({ apis: ["setInterval", "setTimeout"], now: 0 });
    installDom();
  });

  afterEach(async () => {
    await act(async () => {
      if (root) {
        root.unmount();
        root = null;
      }
    });
    mock.timers.reset();
    uninstallDom();
  });

  it("shows Quota OK when no amber rows (EN)", async () => {
    await renderBadge([quota({ id: "a", tool: "Cursor", percent: 10 })], "en");
    const el = badgeEl();
    assert.match(visibleLabel(el), /Quota OK/);
    assert.equal(el.getAttribute("data-amber-count"), "0");
  });

  it("shows Kota normal when no amber rows (TR)", async () => {
    await renderBadge([quota({ id: "a", tool: "Cursor", percent: 10 })], "tr");
    const el = badgeEl();
    assert.match(visibleLabel(el), /Kota normal/);
  });

  it("renders tool name + percent for a single amber row (EN/TR)", async () => {
    const rows = [quota({ id: "c", tool: "Claude", percent: 98, reset: "daily" })];
    await renderBadge(rows, "en");
    assert.match(visibleLabel(badgeEl()), /Claude\s*98%/);
    assert.match(badgeEl().getAttribute("title") || "", /Claude 98%/);

    await renderBadge(rows, "tr");
    assert.match(visibleLabel(badgeEl()), /Claude\s*%98/);
    assert.match(badgeEl().getAttribute("aria-label") || "", /kota eşiğinde/);
  });

  it("rotates every interval through amber rows", async () => {
    const rows = [
      quota({ id: "c", tool: "Claude", percent: 98 }),
      quota({ id: "u", tool: "Cursor", percent: 85 }),
    ];
    await renderBadge(rows, "en");
    assert.equal(badgeEl().getAttribute("data-badge-index"), "0");
    assert.match(visibleLabel(badgeEl()), /Claude/);

    await act(async () => {
      mock.timers.tick(QUOTA_BADGE_ROTATE_MS);
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "1");
    assert.match(visibleLabel(badgeEl()), /Cursor/);

    await act(async () => {
      mock.timers.tick(QUOTA_BADGE_ROTATE_MS);
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "0");
    assert.match(visibleLabel(badgeEl()), /Claude/);
  });

  it("pauses rotation on hover and focus", async () => {
    const rows = [
      quota({ id: "c", tool: "Claude", percent: 98 }),
      quota({ id: "u", tool: "Cursor", percent: 85 }),
    ];
    await renderBadge(rows, "en");
    const el = badgeEl() as HTMLAnchorElement;

    await act(async () => {
      el.dispatchEvent(
        new window.MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body }),
      );
    });
    assert.equal(badgeEl().getAttribute("data-paused"), "1");
    await act(async () => {
      mock.timers.tick(QUOTA_BADGE_ROTATE_MS * 3);
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "0");

    await act(async () => {
      el.dispatchEvent(
        new window.MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }),
      );
    });
    assert.equal(badgeEl().getAttribute("data-paused"), "0");
    await act(async () => {
      mock.timers.tick(QUOTA_BADGE_ROTATE_MS);
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "1");

    await act(async () => {
      el.focus();
    });
    assert.equal(badgeEl().getAttribute("data-paused"), "1");
    await act(async () => {
      mock.timers.tick(QUOTA_BADGE_ROTATE_MS * 2);
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "1");
  });

  it("reduced-motion shows top row plus +N chip", async () => {
    reducedMotion = true;
    uninstallDom();
    installDom();
    const rows = [
      quota({ id: "c", tool: "Claude", percent: 98 }),
      quota({ id: "u", tool: "Cursor", percent: 85 }),
      quota({ id: "g", tool: "Grok", percent: 80 }),
    ];
    await renderBadge(rows, "en");
    assert.match(visibleLabel(badgeEl()), /Claude/);
    assert.match(visibleLabel(badgeEl()), /\+2/);
    const more = document.querySelector("[data-testid='quota-badge-more']");
    assert.ok(more);

    await act(async () => {
      mock.timers.tick(QUOTA_BADGE_ROTATE_MS * 2);
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "0");
  });

  it("resets index when amber row set changes", async () => {
    const initial = [
      quota({ id: "c", tool: "Claude", percent: 98 }),
      quota({ id: "u", tool: "Cursor", percent: 85 }),
    ];
    await renderBadge(initial, "en");
    await act(async () => {
      mock.timers.tick(QUOTA_BADGE_ROTATE_MS);
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "1");

    const next = [
      quota({ id: "g", tool: "Grok", percent: 90 }),
      quota({ id: "c", tool: "Claude", percent: 88 }),
    ];
    await act(async () => {
      root!.render(
        createElement(
          I18nextProvider,
          { i18n },
          createElement(QuotaHeaderBadge, {
            quotas: next,
            LinkComponent: StubLink,
          }),
        ),
      );
    });
    assert.equal(badgeEl().getAttribute("data-badge-index"), "0");
    assert.match(visibleLabel(badgeEl()), /Grok/);
  });

  it("marks tool name with lang=en and no uppercase class", async () => {
    await renderBadge([quota({ id: "c", tool: "Claude", percent: 98 })], "tr");
    const name = badgeEl().querySelector("[lang='en']");
    assert.ok(name);
    assert.equal(name.textContent, "Claude");
    assert.ok(!/\buppercase\b/.test(name.className));
  });
});
