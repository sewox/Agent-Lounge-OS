import { describe, expect, it } from "vitest";
import {
  acceptsCrossPlatformPath,
  formatExperienceLogSummary,
  formatDisplayPath,
  resolveGraphTotals,
} from "@/lib/experience";

describe("formatExperienceLogSummary", () => {
  it("strips markdown dumps and internal TR errors (EX-15)", () => {
    const raw =
      "## Cross-Project Memory ### Tecrübeler\nmemory_bridge hata: repo_path çözümlenemedi\nnodes=2286 edges=7958\nIndexed dispatcher.rs + NATS subjects.";
    const summary = formatExperienceLogSummary(raw);
    expect(summary).not.toMatch(/##\s*Cross-Project Memory/i);
    expect(summary).not.toMatch(/memory_bridge hata:/i);
    expect(summary).toMatch(/Indexed dispatcher\.rs/i);
    expect(summary).toMatch(/nodes=2286 edges=7958/);
  });
});

describe("formatDisplayPath", () => {
  it("preserves cross-platform separators (PATH-01)", () => {
    expect(formatDisplayPath("C:\\Users\\dev\\main.rs")).toBe("C:\\Users\\dev\\main.rs");
    expect(formatDisplayPath("/home/dev/main.rs")).toBe("/home/dev/main.rs");
    expect(formatDisplayPath("mixed/path\\with\\both")).toBe("mixed/path\\with\\both");
    expect(acceptsCrossPlatformPath("C:\\Users\\dev\\main.rs")).toBe(true);
    expect(acceptsCrossPlatformPath("/home/dev/main.rs")).toBe(true);
    expect(acceptsCrossPlatformPath("mixed/path\\with\\both")).toBe(true);
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
    expect(totals.nodes).toBe(2286);
    expect(totals.edges).toBe(7958);
  });
});
