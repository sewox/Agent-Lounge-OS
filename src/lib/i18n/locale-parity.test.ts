import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const LOCALES_ROOT = join(process.cwd(), "src/lib/i18n/locales");

function flattenKeys(obj: Record<string, unknown>, prefix = ""): string[] {
  const keys: string[] = [];
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object" && !Array.isArray(value)) {
      keys.push(...flattenKeys(value as Record<string, unknown>, path));
    } else {
      keys.push(path);
    }
  }
  return keys.sort();
}

describe("i18n locale parity", () => {
  it("every key exists in both en and tr for each namespace", () => {
    const enDir = join(LOCALES_ROOT, "en");
    const trDir = join(LOCALES_ROOT, "tr");
    const enFiles = readdirSync(enDir).filter((f) => f.endsWith(".json")).sort();
    const trFiles = readdirSync(trDir).filter((f) => f.endsWith(".json")).sort();
    assert.deepEqual(enFiles, trFiles, "en/tr namespace file sets must match");

    for (const file of enFiles) {
      const en = JSON.parse(readFileSync(join(enDir, file), "utf8")) as Record<
        string,
        unknown
      >;
      const tr = JSON.parse(readFileSync(join(trDir, file), "utf8")) as Record<
        string,
        unknown
      >;
      const enKeys = flattenKeys(en);
      const trKeys = flattenKeys(tr);
      assert.deepEqual(
        enKeys,
        trKeys,
        `key mismatch in ${file}: missing en=${enKeys.filter((k) => !trKeys.includes(k)).join(",")} missing tr=${trKeys.filter((k) => !enKeys.includes(k)).join(",")}`,
      );
    }
  });
});
