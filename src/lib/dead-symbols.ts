import type { DeadSymbol } from "@/lib/lounge";
import { pathBasename } from "@/lib/lounge";

export type DeadSymbolSort = "name" | "file" | "kind";

export function deadSymbolKey(symbol: DeadSymbol): string {
  return [
    symbol.project_id ?? "",
    symbol.name,
    symbol.kind,
    symbol.file ?? "",
    String(symbol.line ?? ""),
  ].join("|");
}

export function formatSymbolFileLine(symbol: DeadSymbol): string {
  if (!symbol.file?.trim()) {
    return symbol.name;
  }
  const base = pathBasename(symbol.file) || symbol.file;
  return symbol.line != null && symbol.line > 0 ? `${base}:${symbol.line}` : base;
}

export function formatSymbolFullPathLine(symbol: DeadSymbol): string {
  if (!symbol.file?.trim()) {
    return symbol.name;
  }
  return symbol.line != null && symbol.line > 0
    ? `${symbol.file}:${symbol.line}`
    : symbol.file;
}

export function filterDeadSymbols(
  symbols: DeadSymbol[],
  query: string,
  kind: "all" | "unused" | "broken",
  projectId?: string | null,
): DeadSymbol[] {
  const q = query.trim().toLowerCase();
  return symbols.filter((symbol) => {
    if (projectId && symbol.project_id && symbol.project_id !== projectId) {
      return false;
    }
    if (kind !== "all" && symbol.kind !== kind) {
      return false;
    }
    if (!q) {
      return true;
    }
    const hay = [
      symbol.name,
      symbol.kind,
      symbol.file ?? "",
      symbol.detail ?? "",
      symbol.project_id ?? "",
      symbol.last_ref ?? "",
    ]
      .join(" ")
      .toLowerCase();
    return hay.includes(q);
  });
}

export function sortDeadSymbols(symbols: DeadSymbol[], sort: DeadSymbolSort): DeadSymbol[] {
  const rows = [...symbols];
  rows.sort((a, b) => {
    if (sort === "file") {
      return formatSymbolFullPathLine(a).localeCompare(formatSymbolFullPathLine(b));
    }
    if (sort === "kind") {
      return a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name);
    }
    return a.name.localeCompare(b.name);
  });
  return rows;
}
