import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  acceptsCrossPlatformPath,
  formatExperienceLogSummary,
  formatDisplayPath,
  resolveGraphTotals,
} from "@/lib/experience.ts";

describe("formatExperienceLogSummary", () => {
  it("strips markdown dumps and internal TR errors (EX-15)", () => {
    const raw =
      "## Cross-Project Memory ### Tecrübeler\nmemory_bridge hata: repo_path çözümlenemedi\nnodes=2286 edges=7958\nIndexed dispatcher.rs + NATS subjects.";
    const summary = formatExperienceLogSummary(raw);
    assert.doesNotMatch(summary, /##\s*Cross-Project Memory/i);
    assert.doesNotMatch(summary, /memory_bridge hata:/i);
    assert.match(summary, /Indexed dispatcher\.rs/i);
    assert.match(summary, /nodes=2286 edges=7958/);
  });
});

describe("formatDisplayPath", () => {
  it("preserves cross-platform separators (PATH-01)", () => {
    assert.equal(formatDisplayPath("C:\\Users\\dev\\main.rs"), "C:\\Users\\dev\\main.rs");
    assert.equal(formatDisplayPath("/home/dev/main.rs"), "/home/dev/main.rs");
    assert.equal(formatDisplayPath("mixed/path\\with\\both"), "mixed/path\\with\\both");
    assert.equal(acceptsCrossPlatformPath("C:\\Users\\dev\\main.rs"), true);
    assert.equal(acceptsCrossPlatformPath("/home/dev/main.rs"), true);
    assert.equal(acceptsCrossPlatformPath("mixed/path\\with\\both"), true);
  });
});

describe("resolveGraphTotals", () => {
  it("uses backend project COUNT totals, not LIMIT lists (EX-14)", () => {
    const totals = resolveGraphTotals({
      projects: [{ name: "A", nodes: 2286, edges: 7958, files: 10 }],
      semanticMap: {
        projects: [
          {
            name: "A",
            repo_path: "/a",
            files: 10,
            node_count: 400,
            edge_count: 800,
            nodes: [],
            references: [],
            dead: [],
          },
        ],
      },
    });
    assert.equal(totals.nodes, 2286);
    assert.equal(totals.edges, 7958);
  });
});
