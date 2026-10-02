/**
 * Vault IPC mock — separate module to keep tauri-mock.ts merge conflicts small (PR #71).
 * Browser helpers are injected as a string (init scripts cannot import TS modules).
 */
import type { FixtureDataset } from "../fixtures/types";

/** On-demand page counts (not embedded in semanticMap payload). */
export const VAULT_PAGE_COUNTS: Record<string, number> = {
  "Agent-Lounge-OS": 806,
  EchoMind: 220,
  "codebase-memory-mcp": 180,
};

const VAULT_PAGE_PREFIXES: Record<string, string> = {
  "Agent-Lounge-OS": "src",
  EchoMind: "workers",
  "codebase-memory-mcp": "bridge",
};

/** JS source injected before the main Tauri mock (defines window.__QA_VAULT_MOCK__). */
export function vaultMockBrowserSource(pageCounts: Record<string, number> = VAULT_PAGE_COUNTS): string {
  return `
window.__QA_VAULT_PAGE_COUNTS__ = ${JSON.stringify(pageCounts)};
window.__QA_VAULT_PAGE_PREFIXES__ = ${JSON.stringify(VAULT_PAGE_PREFIXES)};
if (typeof window.__QA_VAULT_FAIL__ === "undefined") window.__QA_VAULT_FAIL__ = false;
window.__QA_VAULT_MOCK__ = {
  _pageTotal(f, projectId) {
    const pageCounts = window.__QA_VAULT_PAGE_COUNTS__ || {};
    const prefixes = window.__QA_VAULT_PAGE_PREFIXES__ || {};
    const count = pageCounts[projectId] ?? 0;
    const prefix = prefixes[projectId] ?? "src";
    const paths = new Set();
    for (let i = 0; i < count; i++) {
      paths.add(prefix + "/page_" + String(i).padStart(3, "0") + ".rs");
    }
    const project = (f.semanticMap?.projects || []).find((row) => row.name === projectId);
    for (const node of project?.nodes || []) {
      if (node.file) paths.add(node.file);
    }
    return paths.size;
  },
  listProjects(f) {
    if (window.__QA_VAULT_FAIL__) throw new Error("list_vault_projects unavailable");
    const byName = new Map();
    for (const project of f.projects || []) {
      const exps = (f.experiences || []).filter(
        (row) =>
          (row.project_id || "").toLowerCase() === project.name.toLowerCase() &&
          (row.status ?? "active") === "active",
      );
      const unreviewed = exps.filter((row) => row.reviewed === false).length;
      const lastUpdated =
        exps
          .map((row) => row.updated_at || row.created_at)
          .filter(Boolean)
          .sort()
          .at(-1) ?? null;
      const pageCount = window.__QA_VAULT_MOCK__._pageTotal(f, project.name);
      byName.set(project.name, {
        name: project.name,
        repo_path: project.root_path,
        page_count: pageCount,
        node_count: project.nodes,
        edge_count: project.edges,
        experience_count: exps.length,
        unreviewed_count: unreviewed,
        last_updated: lastUpdated,
        source_type: pageCount > 0 || project.nodes > 0 ? "indexed" : "discovered",
      });
    }
    for (const exp of f.experiences || []) {
      const name = (exp.project_id || "").trim();
      if (!name || byName.has(name)) continue;
      if ((exp.status ?? "active") !== "active") continue;
      const exps = (f.experiences || []).filter(
        (row) =>
          (row.project_id || "").toLowerCase() === name.toLowerCase() &&
          (row.status ?? "active") === "active",
      );
      if (exps.length === 0) continue;
      const unreviewed = exps.filter((row) => row.reviewed === false).length;
      const lastUpdated =
        exps
          .map((row) => row.updated_at || row.created_at)
          .filter(Boolean)
          .sort()
          .at(-1) ?? null;
      byName.set(name, {
        name,
        repo_path: null,
        page_count: 0,
        node_count: 0,
        edge_count: 0,
        experience_count: exps.length,
        unreviewed_count: unreviewed,
        last_updated: lastUpdated,
        source_type: "imported",
      });
    }
    return [...byName.values()];
  },
  listPages(f, args) {
    if (window.__QA_VAULT_FAIL__) throw new Error("list_project_pages unavailable");
    const pageCounts = window.__QA_VAULT_PAGE_COUNTS__ || {};
    const prefixes = window.__QA_VAULT_PAGE_PREFIXES__ || {};
    const projectId = String(args?.projectId ?? args?.project_id ?? "");
    const query = String(args?.query ?? "").trim().toLowerCase();
    const sort = String(args?.sort ?? "path");
    const offset = typeof args?.offset === "number" ? args.offset : 0;
    const limit = typeof args?.limit === "number" ? args.limit : 50;
    const count = pageCounts[projectId] ?? 0;
    const prefix = prefixes[projectId] ?? "src";
    let pages = Array.from({ length: count }, (_, i) => {
      const path = prefix + "/page_" + String(i).padStart(3, "0") + ".rs";
      return {
        path,
        title: path.split("/").at(-1) || path,
        snippet: "sym_" + projectId + "_" + i,
        symbol_count: 1 + (i % 5),
        last_updated: null,
      };
    });
    const project = (f.semanticMap?.projects || []).find((row) => row.name === projectId);
    for (const node of project?.nodes || []) {
      if (!node.file) continue;
      pages.unshift({
        path: node.file,
        title: node.file.replace(/\\\\/g, "/").split("/").filter(Boolean).at(-1) || node.file,
        snippet: node.name,
        symbol_count: node.ref_count || 1,
        last_updated: null,
      });
    }
    const seen = new Set();
    pages = pages.filter((page) => {
      if (seen.has(page.path)) return false;
      seen.add(page.path);
      return true;
    });
    if (query) {
      pages = pages.filter((page) =>
        (page.path + " " + page.title + " " + page.snippet).toLowerCase().includes(query),
      );
    }
    if (sort === "title") pages.sort((a, b) => a.title.localeCompare(b.title));
    else if (sort === "symbols")
      pages.sort((a, b) => b.symbol_count - a.symbol_count || a.path.localeCompare(b.path));
    else if (sort === "updated")
      pages.sort(
        (a, b) =>
          (b.last_updated || "").localeCompare(a.last_updated || "") ||
          a.path.localeCompare(b.path),
      );
    else pages.sort((a, b) => a.path.localeCompare(b.path));
    return {
      project_id: projectId,
      pages: pages.slice(offset, offset + limit),
      total: pages.length,
      offset,
      limit,
    };
  },
};
`;
}

/** Node-side helper for unit tests (mirrors browser mock). */
export function handleVaultMockCommand(
  cmd: string,
  args: Record<string, unknown> | undefined,
  f: FixtureDataset,
  failVault = false,
): { handled: true; value: unknown } | { handled: false } {
  if (cmd !== "list_vault_projects" && cmd !== "list_project_pages") {
    return { handled: false };
  }
  if (failVault) {
    throw new Error(`${cmd} unavailable`);
  }
  // Evaluate the same logic via a quick local implementation.
  if (cmd === "list_vault_projects") {
    const byName = new Map<string, Record<string, unknown>>();
    for (const project of f.projects) {
      const exps = f.experiences.filter(
        (row) =>
          (row.project_id || "").toLowerCase() === project.name.toLowerCase() &&
          (row.status ?? "active") === "active",
      );
      const pageCount = VAULT_PAGE_COUNTS[project.name] ?? project.files ?? 0;
      byName.set(project.name, {
        name: project.name,
        repo_path: project.root_path,
        page_count: pageCount,
        node_count: project.nodes,
        edge_count: project.edges,
        experience_count: exps.length,
        unreviewed_count: exps.filter((row) => row.reviewed === false).length,
        last_updated:
          exps
            .map((row) => row.updated_at || row.created_at)
            .filter(Boolean)
            .sort()
            .at(-1) ?? null,
        source_type: pageCount > 0 || project.nodes > 0 ? "indexed" : "discovered",
      });
    }
    for (const exp of f.experiences) {
      const name = (exp.project_id || "").trim();
      if (!name || byName.has(name)) continue;
      if ((exp.status ?? "active") !== "active") continue;
      const exps = f.experiences.filter(
        (row) =>
          (row.project_id || "").toLowerCase() === name.toLowerCase() &&
          (row.status ?? "active") === "active",
      );
      if (!exps.length) continue;
      byName.set(name, {
        name,
        repo_path: null,
        page_count: 0,
        node_count: 0,
        edge_count: 0,
        experience_count: exps.length,
        unreviewed_count: exps.filter((row) => row.reviewed === false).length,
        last_updated:
          exps
            .map((row) => row.updated_at || row.created_at)
            .filter(Boolean)
            .sort()
            .at(-1) ?? null,
        source_type: "imported",
      });
    }
    return { handled: true, value: [...byName.values()] };
  }
  return { handled: false };
}
