import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  detectInitialLocale,
  detectOsLocale,
  isAppLocale,
  normalizeLocaleTag,
  readStoredLocale,
  writeStoredLocale,
} from "./locale.ts";

describe("locale helpers", () => {
  it("normalizes OS language tags", () => {
    assert.equal(normalizeLocaleTag("tr-TR"), "tr");
    assert.equal(normalizeLocaleTag("en-US"), "en");
    assert.equal(normalizeLocaleTag("de-DE"), null);
    assert.equal(isAppLocale("tr"), true);
    assert.equal(isAppLocale("fr"), false);
  });

  it("prefers stored locale over OS", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => {
        store.set(key, value);
      },
    };
    writeStoredLocale("en", storage);
    assert.equal(readStoredLocale(storage), "en");
    assert.equal(detectInitialLocale(storage, { language: "tr-TR", languages: ["tr-TR"] }), "en");
  });

  it("first launch follows OS language tr-TR → tr", () => {
    const empty = {
      getItem: () => null,
      setItem: () => undefined,
    };
    assert.equal(
      detectInitialLocale(empty, { language: "tr-TR", languages: ["tr-TR", "en-US"] }),
      "tr",
    );
    assert.equal(detectOsLocale({ language: "tr-TR", languages: ["tr-TR"] }), "tr");
  });

  it("falls back to EN when OS language unsupported", () => {
    assert.equal(detectOsLocale({ language: "de-DE", languages: ["de-DE"] }), "en");
    // Without a navigator mock, detectOsLocale returns en in Node.
    assert.equal(detectOsLocale(null), "en");
  });
});
