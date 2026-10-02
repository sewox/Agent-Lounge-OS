import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { LoungeExperience, SemanticMap } from "@/lib/lounge";
import {
  buildVaultProjectRows,
  DEFAULT_VAULT_FILTERS,
  filterAndSortPages,
  filterVaultProjects,
  getServerVaultFiltersSnapshot,
  getVaultFiltersSnapshot,
  pagesFromSemanticProject,
  parseVaultFilters,
  readVaultFilters,
  resetVaultFilters,
  subscribeVaultFilters,
  VAULT_FILTERS_KEY,
  windowSlice,
  writeVaultFilters,
  type VaultProjectRow,
} from "@/lib/vault-projects";

const semanticMap: SemanticMap = {
  projects: [
    {
      name: "Alpha",
      repo_path: "/tmp/alpha",
      files: 3,
      node_count: 4,
      edge_count: 1,
      nodes: [
        { id: "1", name: "foo", kind: "fn", file: "src/a.rs", line: 1, ref_count: 1 },
        { id: "2", name: "bar", kind: "fn", file: "src/b.rs", line: 1, ref_count: 0 },
        { id: "3", name: "baz", kind: "fn", file: "src/a.rs", line: 10, ref_count: 2 },
      ],
      references: [{ from_id: "foo", to_id: "bar", file: "src/c.rs", line: 2 }],
      dead: [],
    },
    {
      name: "Beta",
      repo_path: "/tmp/beta",
      files: 1,
      node_count: 1,
      edge_count: 0,
      nodes: [
        { id: "4", name: "solo", kind: "fn", file: "lib/solo.ts", line: 1, ref_count: 0 },
      ],
      references: [],
      dead: [],
    },
  ],
};

const experiences: LoungeExperience[] = [
  {
    id: "e1",
    type: "experience",
    agent: "kernel",
    project_id: "Alpha",
    adr_summary: "Alpha fix",
    outcome: "success",
    tags: [],
    created_at: "2026-09-28T12:00:00.000Z",
    reviewed: false,
    status: "active",
  },
  {
    id: "e2",
    type: "experience",
    agent: "kernel",
    project_id: "Alpha",
    adr_summary: "Alpha review",
    outcome: "success",
    tags: [],
    created_at: "2026-09-20T12:00:00.000Z",
    reviewed: true,
    status: "active",
  },
  {
    id: "e3",
    type: "experience",
    agent: "kernel",
    project_id: "Gamma",
    adr_summary: "Experience-only project",
    outcome: "partial",
    tags: [],
    created_at: "2026-09-29T08:00:00.000Z",
    reviewed: true,
    status: "active",
  },
];

describe("vault project grouping", () => {
  it("builds one row per project with page and experience counts", () => {
    const rows = buildVaultProjectRows({
      semanticMap,
      projects: [],
      experiences,
    });
    assert.equal(rows.length, 3);
    const alpha = rows.find((r) => r.name === "Alpha");
    assert.ok(alpha);
    // kind=node files only (a.rs, b.rs) — reference-only c.rs excluded
    assert.equal(alpha.pageCount, 2);
    assert.equal(alpha.experienceCount, 2);
    assert.equal(alpha.unreviewedCount, 1);
    assert.equal(alpha.sourceType, "indexed");
    const gamma = rows.find((r) => r.name === "Gamma");
    assert.ok(gamma);
    assert.equal(gamma.pageCount, 0);
    assert.equal(gamma.sourceType, "imported");
  });

  it("filters by name, min pages, recent, review status, source", () => {
    const rows = buildVaultProjectRows({ semanticMap, projects: [], experiences });
    const byName = filterVaultProjects(rows, {
      ...DEFAULT_VAULT_FILTERS,
      nameQuery: "alp",
    });
    assert.deepEqual(
      byName.map((r) => r.name),
      ["Alpha"],
    );

    const minPages = filterVaultProjects(rows, {
      ...DEFAULT_VAULT_FILTERS,
      minPageCount: 2,
    });
    assert.deepEqual(
      minPages.map((r) => r.name),
      ["Alpha"],
    );

    const hasExp = filterVaultProjects(rows, {
      ...DEFAULT_VAULT_FILTERS,
      reviewStatus: "has_experiences",
    });
    assert.ok(hasExp.every((r) => r.experienceCount > 0));

    const unreviewed = filterVaultProjects(rows, {
      ...DEFAULT_VAULT_FILTERS,
      reviewStatus: "unreviewed",
    });
    assert.deepEqual(
      unreviewed.map((r) => r.name),
      ["Alpha"],
    );

    const indexed = filterVaultProjects(rows, {
      ...DEFAULT_VAULT_FILTERS,
      sourceType: "indexed",
    });
    assert.ok(indexed.every((r) => r.sourceType === "indexed"));

    const recent = filterVaultProjects(
      rows,
      { ...DEFAULT_VAULT_FILTERS, recent: "7d" },
      Date.parse("2026-09-30T00:00:00.000Z"),
    );
    assert.ok(recent.some((r) => r.name === "Alpha" || r.name === "Gamma"));
  });

  it("persists filters in storage with reset", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => {
        store.set(k, v);
      },
      removeItem: (k: string) => {
        store.delete(k);
      },
    };
    writeVaultFilters(
      { ...DEFAULT_VAULT_FILTERS, nameQuery: "echo", minPageCount: 5 },
      storage,
    );
    const loaded = readVaultFilters(storage);
    assert.equal(loaded.nameQuery, "echo");
    assert.equal(loaded.minPageCount, 5);
    const reset = resetVaultFilters(storage);
    assert.deepEqual(reset, DEFAULT_VAULT_FILTERS);
    assert.equal(readVaultFilters(storage).nameQuery, "");
  });

  it("parses unknown filter payload safely", () => {
    assert.deepEqual(parseVaultFilters(null), DEFAULT_VAULT_FILTERS);
    assert.deepEqual(parseVaultFilters({ nameQuery: 12, recent: "nope" }), {
      ...DEFAULT_VAULT_FILTERS,
    });
  });

  it("getVaultFiltersSnapshot tolerates corrupt localStorage JSON", () => {
    const prev = globalThis.window;
    const store = new Map<string, string>();
    store.set(VAULT_FILTERS_KEY, "{not-json");
    // @ts-expect-error test stub
    globalThis.window = {
      localStorage: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => {
          store.set(k, v);
        },
        removeItem: (k: string) => {
          store.delete(k);
        },
      },
      addEventListener: () => {},
      removeEventListener: () => {},
    };
    try {
      const snap = getVaultFiltersSnapshot();
      assert.deepEqual(snap, DEFAULT_VAULT_FILTERS);
      assert.deepEqual(getServerVaultFiltersSnapshot(), DEFAULT_VAULT_FILTERS);
      const unsub = subscribeVaultFilters(() => {});
      unsub();
    } finally {
      // @ts-expect-error restore
      globalThis.window = prev;
    }
  });
});

describe("project page list", () => {
  it("groups nodes into unique pages and supports search/sort", () => {
    const pages = pagesFromSemanticProject(semanticMap.projects[0]);
    assert.equal(pages.length, 2);
    assert.equal(pages.find((p) => p.path === "src/a.rs")?.symbolCount, 2);

    const searched = filterAndSortPages(pages, "foo", "symbols");
    assert.equal(searched.length, 1);
    assert.equal(searched[0].path, "src/a.rs");

    const byTitle = filterAndSortPages(pages, "", "title");
    assert.equal(byTitle[0].title, "a.rs");
  });

  it("windowSlice virtualizes large lists", () => {
    const items = Array.from({ length: 800 }, (_, i) => i);
    const win = windowSlice(items, 400, 200, 28, 2);
    assert.ok(win.totalHeight === 800 * 28);
    assert.ok(win.end - win.start < 30);
    assert.ok(win.start >= 0);
  });
});

describe("vault project row shape", () => {
  it("prefers aggregate payload when provided", () => {
    const aggregates: VaultProjectRow[] = [
      {
        name: "OnlyAgg",
        repoPath: "/x",
        pageCount: 12,
        nodeCount: 1,
        edgeCount: 0,
        experienceCount: 0,
        unreviewedCount: 0,
        lastUpdated: null,
        sourceType: "indexed",
      },
    ];
    const rows = buildVaultProjectRows({
      semanticMap,
      projects: [],
      experiences: [],
      aggregates,
    });
    assert.equal(rows.length, 1);
    assert.equal(rows[0].name, "OnlyAgg");
  });
});
