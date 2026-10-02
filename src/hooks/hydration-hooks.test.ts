/**
 * StrictMode + jsdom coverage for usePlatform / useIsTauri.
 * Proves detection still works when React 19 double-invokes effects/renders
 * (the old module-level `hydrated` flag broke that path).
 */
import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";
import { act, createElement, StrictMode } from "react";
import { createRoot, hydrateRoot } from "react-dom/client";
import { renderToString } from "react-dom/server";
import { JSDOM } from "jsdom";
import { useIsTauri } from "../hooks/use-is-tauri.ts";
import { usePlatform } from "../hooks/use-platform.ts";
import {
  SSR_DEFAULT_PLATFORM,
  type LoungePlatform,
} from "../lib/platform.ts";

type NavigatorStub = {
  platform: string;
  userAgent: string;
  userAgentData?: { platform?: string };
};

type HostWindow = Window &
  typeof globalThis & {
    __TAURI_INTERNALS__?: Record<string, unknown>;
  };

const PLATFORM_CASES: Array<{
  id: LoungePlatform;
  navigator: NavigatorStub;
}> = [
  {
    id: "macos",
    navigator: {
      platform: "MacIntel",
      userAgent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
      userAgentData: { platform: "macOS" },
    },
  },
  {
    id: "windows",
    navigator: {
      platform: "Win32",
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
      userAgentData: { platform: "Windows" },
    },
  },
  {
    id: "linux",
    navigator: {
      platform: "Linux x86_64",
      userAgent:
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
      userAgentData: { platform: "Linux" },
    },
  },
];

let dom: JSDOM | null = null;
const saved: {
  window?: typeof globalThis.window;
  document?: typeof globalThis.document;
  navigator?: typeof globalThis.navigator;
  IS_REACT_ACT_ENVIRONMENT?: boolean;
} = {};

function installDom(nav: NavigatorStub, withTauri: boolean) {
  uninstallDom();

  dom = new JSDOM("<!DOCTYPE html><html><body><div id='root'></div></body></html>", {
    url: "http://127.0.0.1/",
    pretendToBeVisual: true,
  });

  const win = dom.window as unknown as HostWindow;
  Object.defineProperty(win.navigator, "platform", {
    configurable: true,
    get: () => nav.platform,
  });
  Object.defineProperty(win.navigator, "userAgent", {
    configurable: true,
    get: () => nav.userAgent,
  });
  Object.defineProperty(win.navigator, "userAgentData", {
    configurable: true,
    get: () => nav.userAgentData,
  });

  if (withTauri) {
    win.__TAURI_INTERNALS__ = { mock: true };
  } else {
    delete win.__TAURI_INTERNALS__;
  }

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

function PlatformProbe() {
  const platform = usePlatform();
  return createElement("span", { "data-platform": platform }, platform);
}

function TauriProbe() {
  const tauriHost = useIsTauri();
  return createElement("span", { "data-tauri": String(tauriHost) }, String(tauriHost));
}

async function renderStrict(node: ReturnType<typeof createElement>) {
  const rootEl = document.getElementById("root");
  assert.ok(rootEl);
  const root = createRoot(rootEl);
  await act(async () => {
    root.render(createElement(StrictMode, null, node));
  });
  return { root, rootEl };
}

describe("usePlatform / useIsTauri under React.StrictMode", () => {
  afterEach(() => {
    uninstallDom();
  });

  for (const platformCase of PLATFORM_CASES) {
    it(`detects ${platformCase.id} platform after mount (StrictMode)`, async () => {
      installDom(platformCase.navigator, false);
      const { root, rootEl } = await renderStrict(createElement(PlatformProbe));

      const node = rootEl.querySelector("[data-platform]");
      assert.ok(node);
      assert.equal(node.getAttribute("data-platform"), platformCase.id);
      assert.equal(node.textContent, platformCase.id);

      await act(async () => {
        root.unmount();
      });
    });

    it(`detects Tauri host on ${platformCase.id} under StrictMode`, async () => {
      installDom(platformCase.navigator, true);
      const { root, rootEl } = await renderStrict(createElement(TauriProbe));

      const node = rootEl.querySelector("[data-tauri]");
      assert.ok(node);
      assert.equal(node.getAttribute("data-tauri"), "true");
      assert.equal(node.textContent, "true");

      await act(async () => {
        root.unmount();
      });
    });

    it(`stays non-Tauri on ${platformCase.id} browser host under StrictMode`, async () => {
      installDom(platformCase.navigator, false);
      const { root, rootEl } = await renderStrict(createElement(TauriProbe));

      const node = rootEl.querySelector("[data-tauri]");
      assert.ok(node);
      assert.equal(node.getAttribute("data-tauri"), "false");

      await act(async () => {
        root.unmount();
      });
    });
  }

  it("SSR snapshot stays on default platform; hydrate then adopts client OS", async () => {
    uninstallDom();
    const html = renderToString(createElement(StrictMode, null, createElement(PlatformProbe)));
    assert.match(html, new RegExp(`data-platform="${SSR_DEFAULT_PLATFORM}"`));

    installDom(PLATFORM_CASES[0]!.navigator, true);
    const rootEl = document.getElementById("root");
    assert.ok(rootEl);
    rootEl.innerHTML = html;

    const hydrationErrors: string[] = [];
    const prevError = console.error;
    console.error = (...args: unknown[]) => {
      hydrationErrors.push(args.map(String).join(" "));
    };

    try {
      let root: ReturnType<typeof hydrateRoot>;
      await act(async () => {
        root = hydrateRoot(
          rootEl,
          createElement(StrictMode, null, createElement(PlatformProbe)),
        );
      });

      // After hydration, client snapshot (macOS) must replace SSR default.
      await act(async () => {
        await Promise.resolve();
      });

      const node = rootEl.querySelector("[data-platform]");
      assert.ok(node);
      assert.equal(node.getAttribute("data-platform"), "macos");

      const noise = hydrationErrors.filter((text) => /hydrat/i.test(text));
      assert.deepEqual(noise, []);

      await act(async () => {
        root!.unmount();
      });
    } finally {
      console.error = prevError;
    }
  });

  it("SSR Tauri snapshot stays false; hydrate then detects __TAURI_INTERNALS__", async () => {
    uninstallDom();
    const html = renderToString(createElement(StrictMode, null, createElement(TauriProbe)));
    assert.match(html, /data-tauri="false"/);

    installDom(PLATFORM_CASES[0]!.navigator, true);
    const rootEl = document.getElementById("root");
    assert.ok(rootEl);
    rootEl.innerHTML = html;

    let root: ReturnType<typeof hydrateRoot>;
    await act(async () => {
      root = hydrateRoot(
        rootEl,
        createElement(StrictMode, null, createElement(TauriProbe)),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    const node = rootEl.querySelector("[data-tauri]");
    assert.ok(node);
    assert.equal(node.getAttribute("data-tauri"), "true");

    await act(async () => {
      root!.unmount();
    });
  });
});
