"use client";

import { useMemo, useState } from "react";
import { Icon } from "@/components/icons";
import { IndexEmptyState } from "@/components/index-empty-state";
import { Pager } from "@/components/ui";
import {
  pageCount,
  pageSlice,
  pathBasename,
  shortSymbolName,
  type AstNode,
  type CodeReference,
  type ProjectSummary,
  type SemanticMap as SemanticMapData,
  type SemanticMapSelection,
} from "@/lib/lounge";

const NODE_PAGE = 12;
const REF_PREVIEW = 6;

type MapRow = {
  key: string;
  selection: SemanticMapSelection;
  label: string;
  meta: string;
  kind: "project" | "node" | "ref";
  depth: number;
};

type SemanticMapProps = {
  semanticMap: SemanticMapData;
  projects: ProjectSummary[];
  fileTotal: number;
  edgeTotal: number;
  selected: SemanticMapSelection | null;
  onSelect: (next: SemanticMapSelection | null) => void;
};

function buildRows(
  semanticMap: SemanticMapData,
  projects: ProjectSummary[],
  selected: SemanticMapSelection | null,
): MapRow[] {
  if (semanticMap.projects.length > 0) {
    const rows: MapRow[] = [];
    for (const project of semanticMap.projects) {
      rows.push({
        key: `proj:${project.name}:${project.repo_path}`,
        selection: {
          id: project.name,
          name: project.name,
          kind: "project",
          file: null,
          project: project.name,
        },
        label: project.name || "unnamed",
        meta: `${project.edge_count} edges · ${project.node_count} nodes`,
        kind: "project",
        depth: 0,
      });
      const byId = new Map<string, AstNode>();
      for (const node of project.nodes) {
        if (node.id) {
          byId.set(node.id, node);
        }
        if (node.name) {
          byId.set(node.name, node);
        }
      }
      for (const node of project.nodes) {
        const row = nodeRow(project.name, node);
        rows.push(row);
        const isOpen =
          selected != null &&
          selected.project === project.name &&
          (selected.id === row.selection.id ||
            selected.name === row.selection.name ||
            selected.id === node.id ||
            selected.name === node.name);
        if (!isOpen) {
          continue;
        }
        for (const edge of outgoingRefs(node, project.references).slice(0, REF_PREVIEW)) {
          const target = byId.get(edge.to_id) ?? byId.get(edge.callee ?? "");
          const calleeLabel =
            target?.name ||
            edge.callee ||
            shortSymbolName(edge.to_id) ||
            edge.to_id ||
            "unknown";
          rows.push({
            key: `ref:${project.name}:${edge.from_id}->${edge.to_id}:${edge.line ?? ""}`,
            selection: {
              id: edge.to_id || target?.id || edge.to_id,
              name: calleeLabel,
              kind: target?.kind || "function",
              file: edge.file ?? target?.file ?? null,
              project: project.name,
            },
            label: `→ ${calleeLabel}`,
            meta: edge.file
              ? `CALLS · ${pathBasename(edge.file) || edge.file}`
              : "CALLS",
            kind: "ref",
            depth: 2,
          });
        }
      }
    }
    return rows;
  }

  if (projects.length > 0) {
    return projects.map((row) => ({
      key: `proj:${row.name}:${row.root_path ?? ""}`,
      selection: {
        id: row.name,
        name: row.name || "unnamed",
        kind: "project",
        file: null,
        project: row.name,
      },
      label: row.name || "unnamed",
      meta: `${row.edges} edges · ${row.nodes} nodes`,
      kind: "project" as const,
      depth: 0,
    }));
  }

  return [];
}

function nodeRow(project: string, node: AstNode): MapRow {
  const file = pathBasename(node.file) || node.file || "—";
  return {
    key: `node:${project}:${node.id || node.name}:${node.line ?? ""}`,
    selection: {
      id: node.id || node.name,
      name: node.name || node.id,
      kind: node.kind || "node",
      file: node.file,
      project,
    },
    label: node.name || node.id || "unnamed",
    meta: `${node.kind || "node"} · ${file} · ${node.ref_count} refs`,
    kind: "node",
    depth: 1,
  };
}

function outgoingRefs(node: AstNode, references: CodeReference[]): CodeReference[] {
  const ids = new Set(
    [node.id, node.name, shortSymbolName(node.id)].filter((token) => Boolean(token)),
  );
  return references.filter((edge) => {
    const from = edge.from_id || edge.caller || "";
    return ids.has(from) || ids.has(shortSymbolName(from));
  });
}

function selectionKey(row: SemanticMapSelection | null): string | null {
  if (!row) {
    return null;
  }
  return `${row.project ?? ""}|${row.id}|${row.file ?? ""}|${row.name}`;
}

export function SemanticMap({
  semanticMap,
  projects,
  fileTotal,
  edgeTotal,
  selected,
  onSelect,
}: SemanticMapProps) {
  const rows = useMemo(
    () => buildRows(semanticMap, projects, selected),
    [semanticMap, projects, selected],
  );
  const [page, setPage] = useState(0);
  const pages = pageCount(rows.length, NODE_PAGE);
  const safePage = Math.min(page, pages - 1);
  const visible = pageSlice(rows, safePage, NODE_PAGE);
  const active = selectionKey(selected);
  const repoCount = semanticMap.projects.length || projects.length;

  if (rows.length === 0) {
    return (
      <div className="flex min-h-0 w-full flex-1 flex-col overflow-hidden">
        <IndexEmptyState detail="No data found." className="min-h-0 flex-1" />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 w-full flex-1 flex-col overflow-hidden">
      <div className="mb-2 flex shrink-0 items-center justify-between font-body text-meta font-semibold tracking-label text-outline uppercase">
        <span>Indexed Files</span>
        <span className="text-on-surface-variant">
          {fileTotal > 0
            ? `${fileTotal} files · ${edgeTotal} edges`
            : `${edgeTotal} edges · ${repoCount} repos`}
        </span>
      </div>
      <div className="min-h-0 flex-1 space-y-1 overflow-auto font-body text-body">
        {visible.map((row) => {
          const isActive = active === selectionKey(row.selection);
          const pad =
            row.depth === 0
              ? ""
              : row.depth === 1
                ? "ml-1.5 border-l border-outline-variant/60 pl-3.5"
                : "ml-3 border-l border-outline-variant/40 pl-3.5";
          return (
            <button
              key={row.key}
              type="button"
              onClick={() => onSelect(isActive ? null : row.selection)}
              className={`flex w-full items-center justify-between gap-2 rounded px-1.5 py-1.5 text-left transition-colors ${pad} ${
                isActive
                  ? "bg-primary-container/25 text-primary"
                  : "text-on-surface hover:bg-surface-container-high/80"
              }`}
            >
              <span className="flex min-w-0 items-center gap-1">
                {row.kind === "project" ? (
                  <Icon name="folder" className="h-3 w-3 shrink-0" />
                ) : row.kind === "ref" ? (
                  <span className="shrink-0 text-meta text-outline">↳</span>
                ) : (
                  <Icon name="tree" className="h-3 w-3 shrink-0 text-on-surface-variant" />
                )}
                <span className="truncate font-medium font-mono">{row.label}</span>
              </span>
              <span className="shrink-0 font-mono text-meta text-outline">{row.meta}</span>
            </button>
          );
        })}
      </div>
      <div className="mt-1.5 flex shrink-0 items-center justify-between border-t border-outline-variant/40 pt-1.5 font-body text-meta text-outline">
        <span>{selected ? `seçili: ${selected.name}` : `${rows.length} düğüm / ref`}</span>
        <Pager page={safePage} pages={pages} total={rows.length} onPage={setPage} />
      </div>
    </div>
  );
}
