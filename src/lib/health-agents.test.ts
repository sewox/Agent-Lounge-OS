import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  AGENT_MCP_STATUSES,
  containsUnmaskedHome,
  countDeadByCategory,
  deadSymbolReason,
  filterDeadSymbolsUi,
  isScanStale,
  maskAgentEntryForUi,
  maskHomePathForUi,
  severityToTone,
  statusIconName,
  statusLabelKey,
} from "./health-agents.ts";

const here = dirname(fileURLToPath(import.meta.url));

function loadJson(rel: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(here, rel), "utf8")) as Record<string, unknown>;
}

describe("health-agents status mapping", () => {
  it("covers all 8 statuses with distinct label keys", () => {
    assert.equal(AGENT_MCP_STATUSES.length, 8);
    const keys = new Set(AGENT_MCP_STATUSES.map(statusLabelKey));
    assert.equal(keys.size, 8);
  });

  it("maps severity to StatusPill tones", () => {
    assert.equal(severityToTone("ok"), "ok");
    assert.equal(severityToTone("info"), "info");
    assert.equal(severityToTone("warning"), "warn");
    assert.equal(severityToTone("warn"), "warn");
    assert.equal(severityToTone("other"), "neutral");
  });

  it("uses a distinct icon for unverified vs other statuses", () => {
    assert.equal(statusIconName("unverified"), "link-off");
    assert.notEqual(statusIconName("unverified"), statusIconName("ok"));
    assert.notEqual(statusIconName("unverified"), statusIconName("missing"));
  });

  it("unverified i18n label differs from source_unverified approval label (EN+TR)", () => {
    const enHealth = loadJson("./i18n/locales/en/health.json");
    const enApprovals = loadJson("./i18n/locales/en/approvals.json");
    const trHealth = loadJson("./i18n/locales/tr/health.json");
    const trApprovals = loadJson("./i18n/locales/tr/approvals.json");
    const enStatus = (enHealth.status as Record<string, string>).unverified;
    const trStatus = (trHealth.status as Record<string, string>).unverified;
    const enApproval = String(enApprovals.sourceUnverified);
    const trApproval = String(trApprovals.sourceUnverified);
    assert.equal(enStatus, "Not verified (remote URL)");
    assert.equal(trStatus, "Doğrulanmadı (uzak URL)");
    assert.notEqual(enStatus, enApproval);
    assert.notEqual(trStatus, trApproval);
    assert.match(enApproval, /Unverified source/i);
    assert.match(trApproval, /Doğrulanmamış kaynak/i);
  });
});

describe("health-agents path masking", () => {
  it("masks macOS / Linux / Windows home prefixes", () => {
    assert.equal(maskHomePathForUi("/Users/sercan/.cursor/mcp.json"), "~/.cursor/mcp.json");
    assert.equal(maskHomePathForUi("/home/sercan/.grok/mcp.json"), "~/.grok/mcp.json");
    assert.equal(
      maskHomePathForUi("C:\\Users\\sercan\\AppData\\Roaming\\Claude\\x.json"),
      "~/AppData/Roaming/Claude/x.json",
    );
    assert.equal(maskHomePathForUi("~/already/masked"), "~/already/masked");
    assert.equal(
      maskHomePathForUi("url-only; path=/Users/sercan/.cursor/mcp.json"),
      "url-only; path=~/.cursor/mcp.json",
    );
  });

  it("maskAgentEntryForUi is a second defence on unmasked fixtures", () => {
    const masked = maskAgentEntryForUi({
      client: "cursor",
      status: "unverified",
      severity: "info",
      config_path: "/Users/sercan/.cursor/mcp.json",
      command: "C:\\Users\\sercan\\bin\\lounge-mcp.exe",
      detail: "under /home/sercan/.cursor",
    });
    assert.equal(masked.config_path, "~/.cursor/mcp.json");
    assert.ok(masked.command?.startsWith("~/"));
    assert.ok(!containsUnmaskedHome(masked.config_path ?? ""));
    assert.ok(!containsUnmaskedHome(masked.command ?? ""));
    assert.ok(!containsUnmaskedHome(masked.detail ?? ""));
  });
});

describe("health-agents dead filters", () => {
  const rows = [
    {
      name: "a",
      kind: "unused",
      severity: "Kritik",
      project_id: "P1",
      detail: "x",
    },
    {
      name: "b",
      kind: "broken",
      severity: "Potansiyel",
      project_id: "P1",
      detail: "y",
    },
    {
      name: "c",
      kind: "unused",
      severity: "Potansiyel",
      project_id: "P2",
      detail: "z",
    },
  ];

  it("counts categories and confidence", () => {
    assert.deepEqual(countDeadByCategory(rows), {
      total: 3,
      unused: 2,
      broken: 1,
      kritik: 1,
      potansiyel: 2,
    });
  });

  it("filters by category and confidence", () => {
    const unused = filterDeadSymbolsUi(rows, {
      query: "",
      category: "unused",
      confidence: "all",
    });
    assert.equal(unused.length, 2);
    const kritik = filterDeadSymbolsUi(rows, {
      query: "",
      category: "all",
      confidence: "Kritik",
    });
    assert.equal(kritik.length, 1);
    assert.equal(kritik[0]?.name, "a");
  });

  it("builds plain-language reasons", () => {
    const reason = deadSymbolReason(rows[0]!, {
      unused: "Unused symbol",
      broken: "Broken reference",
      kritik: "Critical confidence",
      potansiyel: "Potential confidence",
      fallback: "Flagged",
    });
    assert.match(reason, /Unused/);
    assert.match(reason, /Critical/);
    assert.match(reason, /x/);
  });

  it("detects stale scans from partial or old indexed_at", () => {
    assert.equal(isScanStale({ partial: true, indexed_at: new Date().toISOString() }), true);
    assert.equal(
      isScanStale({ partial: false, indexed_at: "2020-01-01T00:00:00.000Z" }, Date.parse("2026-10-17")),
      true,
    );
    assert.equal(
      isScanStale({ partial: false, indexed_at: new Date().toISOString() }),
      false,
    );
  });
});
