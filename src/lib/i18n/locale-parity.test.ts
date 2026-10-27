import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const LOCALES_ROOT = join(process.cwd(), "src/lib/i18n/locales");

/**
 * Leaf keys (or exact values) allowed to remain identical across EN/TR.
 * Scoped to universal brand tokens, protocol names, and technical identifiers —
 * never user-facing prose that should differ by locale.
 */
const TECHNICAL_IDENTICAL_ALLOWLIST = new Set<string>([
  // Brand / product
  "shell.brand",
  "shell.titles.default",
  "AL-OS CORE",
  "Lounge",
  // Locale switch chrome
  "shell.localeEn",
  "shell.localeTr",
  "EN",
  "TR",
  // Protocol / service / product tokens
  "CBM",
  "Platform",
  "Global",
  "Model",
  "OS",
  "PID",
  "Heartbeat",
  "ok",
  "nats",
  "nats · supervisor",
  "DecisionGate",
  "Token",
  "api",
  "Amber Alert",
  "Amber Alert · {{tools}}",
  "AMBER {{percent}}%",
  "+{{count}}",
  "VS Code",
  "Cursor",
  "Program",
  "Test",
  "Graph UI",
  "Port",
  "UI port",
  "task.*",
  "exp.*",
  "NATS / 60s",
  "memory_bridge",
  "memory bridge + sqlite",
  "WHISPER",
  "last_ref",
  "{{projectCount}} / {{totalCount}}",
  // Namespace.key paths for the above
  "fleet.colHeartbeat",
  "fleet.colPid",
  "fleet.detailHeartbeat",
  "fleet.footerSupervisor",
  "fleet.statusOk",
  "fleet.endpointNats",
  "fleet.decisionGate",
  "fleet.remoteAccessTokenLabel",
  "health.lastRef",
  "health.agents.sectionCbm",
  "health.agents.sectionPlatform",
  "health.agents.connectScopeGlobal",
  "health.agents.titleModelName",
  "health.agents.cbmPort",
  "health.agents.platformOs",
  "quotas.amberAlert",
  "quotas.filterApi",
  "quotas.amberAlertShort",
  "settings.editorPreset_vscode",
  "settings.editorPreset_cursor",
  "settings.editorCustomProgram",
  "settings.editorTest",
  "settings.graphUiTitle",
  "settings.graphPort",
  "shell.layaReadyWithDevice",
  "shell.quotaAmber",
  "shell.quotaBadgeMore",
  "{{running}} · {{device}}",
  "stream.filterTask",
  "stream.filterExp",
  "stream.kpiNats60s",
  "stream.kpiMemoryBridge",
  "vault.experienceCountScoped",
  "vault.backendSourceLabel",
  "vault.whisper",
]);

function flattenEntries(
  obj: Record<string, unknown>,
  prefix = "",
): Array<{ path: string; value: string }> {
  const out: Array<{ path: string; value: string }> = [];
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object" && !Array.isArray(value)) {
      out.push(...flattenEntries(value as Record<string, unknown>, path));
    } else {
      out.push({ path, value: String(value ?? "") });
    }
  }
  return out.sort((a, b) => a.path.localeCompare(b.path));
}

function flattenKeys(obj: Record<string, unknown>, prefix = ""): string[] {
  return flattenEntries(obj, prefix).map((e) => e.path);
}

function placeholders(value: string): string[] {
  const found = value.match(/\{\{\s*[\w.]+\s*\}\}/g) ?? [];
  return found.map((p) => p.replace(/\s+/g, "")).sort();
}

function isAllowlisted(nsKey: string, value: string): boolean {
  return (
    TECHNICAL_IDENTICAL_ALLOWLIST.has(nsKey) ||
    TECHNICAL_IDENTICAL_ALLOWLIST.has(value)
  );
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

  it("interpolation placeholders match for every leaf key", () => {
    const enDir = join(LOCALES_ROOT, "en");
    const trDir = join(LOCALES_ROOT, "tr");
    const enFiles = readdirSync(enDir).filter((f) => f.endsWith(".json")).sort();

    for (const file of enFiles) {
      const ns = file.replace(/\.json$/, "");
      const en = JSON.parse(readFileSync(join(enDir, file), "utf8")) as Record<
        string,
        unknown
      >;
      const tr = JSON.parse(readFileSync(join(trDir, file), "utf8")) as Record<
        string,
        unknown
      >;
      const enEntries = flattenEntries(en);
      const trMap = new Map(flattenEntries(tr).map((e) => [e.path, e.value]));
      for (const { path, value } of enEntries) {
        const trValue = trMap.get(path) ?? "";
        assert.deepEqual(
          placeholders(value),
          placeholders(trValue),
          `placeholder mismatch ${ns}.${path}: en=${value} tr=${trValue}`,
        );
      }
    }
  });

  it("TR leaf values differ from EN unless allowlisted technical tokens", () => {
    const enDir = join(LOCALES_ROOT, "en");
    const trDir = join(LOCALES_ROOT, "tr");
    const enFiles = readdirSync(enDir).filter((f) => f.endsWith(".json")).sort();
    const identical: string[] = [];

    for (const file of enFiles) {
      const ns = file.replace(/\.json$/, "");
      const en = JSON.parse(readFileSync(join(enDir, file), "utf8")) as Record<
        string,
        unknown
      >;
      const tr = JSON.parse(readFileSync(join(trDir, file), "utf8")) as Record<
        string,
        unknown
      >;
      const enEntries = flattenEntries(en);
      const trMap = new Map(flattenEntries(tr).map((e) => [e.path, e.value]));
      for (const { path, value } of enEntries) {
        const trValue = trMap.get(path) ?? "";
        if (value === trValue) {
          const nsKey = `${ns}.${path}`;
          if (!isAllowlisted(nsKey, value)) {
            identical.push(`${nsKey}=${JSON.stringify(value)}`);
          }
        }
      }
    }

    assert.deepEqual(
      identical,
      [],
      `TR==EN leaf keys not in TECHNICAL_IDENTICAL_ALLOWLIST:\n${identical.join("\n")}`,
    );
  });

  it("covers palette footer + onboarding selectGroup (TR/EN)", () => {
    const enShell = JSON.parse(
      readFileSync(join(LOCALES_ROOT, "en/shell.json"), "utf8"),
    ) as Record<string, string>;
    const trShell = JSON.parse(
      readFileSync(join(LOCALES_ROOT, "tr/shell.json"), "utf8"),
    ) as Record<string, string>;
    const enOn = JSON.parse(
      readFileSync(join(LOCALES_ROOT, "en/onboarding.json"), "utf8"),
    ) as Record<string, string>;
    const trOn = JSON.parse(
      readFileSync(join(LOCALES_ROOT, "tr/onboarding.json"), "utf8"),
    ) as Record<string, string>;

    assert.ok(enShell.paletteFooter?.includes("select"));
    assert.ok(enShell.paletteFooter?.includes("run"));
    assert.ok(!/seç|çalıştır/.test(enShell.paletteFooter ?? ""));
    assert.ok(trShell.paletteFooter?.includes("seç"));
    assert.ok(trShell.paletteFooter?.includes("çalıştır"));
    assert.ok(!/\bselect\b/i.test(trShell.paletteFooter ?? ""));

    assert.equal(enOn.selectGroup, "Select group");
    assert.equal(trOn.selectGroup, "Grubu seç");
    assert.ok(enShell.notInstalled);
    assert.ok(trShell.notInstalled);
    assert.ok(enShell.serviceOptional);
    assert.ok(trShell.serviceOptional);
    assert.equal(enShell.overview, "Overview");
    assert.equal(trShell.overview, "Genel Bakış");
    assert.equal(enShell.running, "Running");
    assert.equal(trShell.running, "Çalışıyor");
    assert.equal(enShell.checking, "Checking…");
    assert.equal(trShell.checking, "Kontrol ediliyor…");
  });
});
