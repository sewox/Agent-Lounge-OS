"use client";

import { invoke } from "@tauri-apps/api/core";
import { useRouter } from "next/navigation";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useLounge } from "@/components/lounge-provider";
import {
  isTauri,
  MOCK_EXPERIENCES,
  MOCK_NODES,
  type AstNode,
  type LoungeExperience,
} from "@/lib/lounge";

type PaletteItem = {
  id: string;
  group: string;
  title: string;
  subtitle: string;
  disabled?: boolean;
  hint?: string;
  run: () => void | Promise<void>;
};

type GrokTestResult = {
  task_id: string;
  subject: string;
  target_agent: string;
  chain_label: string;
};

function matchesQuery(hay: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) {
    return true;
  }
  return hay.toLowerCase().includes(q);
}

function mockSearchExperiences(query: string, limit = 8): LoungeExperience[] {
  const q = query.trim().toLowerCase();
  const rows = !q
    ? MOCK_EXPERIENCES
    : MOCK_EXPERIENCES.filter((row) =>
        `${row.project_id} ${row.adr_summary} ${row.agent} ${row.tags.join(" ")}`
          .toLowerCase()
          .includes(q),
      );
  return rows.slice(0, limit);
}

function mockSearchNodes(query: string, limit = 6): AstNode[] {
  const q = query.trim().toLowerCase();
  const rows = MOCK_NODES.flatMap((node) => {
    const projectHit = !q || node.name.toLowerCase().includes(q);
    const modules = node.modules.filter((mod) => !q || mod.toLowerCase().includes(q));
    const out: AstNode[] = [];
    if (projectHit) {
      out.push({
        id: `mock:${node.name}`,
        name: node.name,
        kind: "project",
        file: null,
        line: null,
        ref_count: node.edges,
      });
    }
    for (const mod of modules) {
      out.push({
        id: `mock:${node.name}:${mod}`,
        name: mod,
        kind: "file",
        file: mod,
        line: null,
        ref_count: 1,
      });
    }
    return out;
  });
  return rows.slice(0, limit);
}

export function CommandPalette() {
  const router = useRouter();
  const {
    experiences,
    projects,
    semanticMap,
    selectedProject,
    switchProject,
    focusExperience,
    setQuery: setVaultQuery,
    openCommandPalette,
    setOpenCommandPalette,
  } = useLounge();

  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [remoteExperiences, setRemoteExperiences] = useState<LoungeExperience[]>([]);
  const [remoteNodes, setRemoteNodes] = useState<AstNode[]>([]);
  const [searching, setSearching] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [grokBusy, setGrokBusy] = useState(false);
  const [wasOpen, setWasOpen] = useState(openCommandPalette);
  const inputRef = useRef<HTMLInputElement>(null);
  const mockMode = !isTauri();

  if (wasOpen !== openCommandPalette) {
    setWasOpen(openCommandPalette);
    if (openCommandPalette) {
      setQuery("");
      setActive(0);
      setStatus(null);
    }
  }

  const close = useCallback(() => {
    setOpenCommandPalette(false);
    setQuery("");
    setActive(0);
    setStatus(null);
  }, [setOpenCommandPalette]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const meta = event.metaKey || event.ctrlKey;
      if (meta && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpenCommandPalette((open) => !open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setOpenCommandPalette]);

  useEffect(() => {
    if (!openCommandPalette) {
      return;
    }
    const id = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => window.clearTimeout(id);
  }, [openCommandPalette]);

  useEffect(() => {
    if (!openCommandPalette) {
      return;
    }
    let cancelled = false;
    const q = query.trim();
    const timer = window.setTimeout(() => {
      void (async () => {
        if (!isTauri()) {
          setRemoteExperiences(mockSearchExperiences(q));
          setRemoteNodes(mockSearchNodes(q));
          setSearching(false);
          return;
        }
        setSearching(true);
        try {
          const [expRows, nodeRows] = await Promise.all([
            invoke<LoungeExperience[]>("search_experiences", {
              query: q,
              limit: 8,
            }).catch(() => [] as LoungeExperience[]),
            q
              ? invoke<AstNode[]>("search_index_nodes", {
                  query: q,
                  limit: 6,
                }).catch(() => [] as AstNode[])
              : Promise.resolve([] as AstNode[]),
          ]);
          if (!cancelled) {
            setRemoteExperiences(expRows);
            setRemoteNodes(nodeRows);
          }
        } finally {
          if (!cancelled) {
            setSearching(false);
          }
        }
      })();
    }, 120);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [openCommandPalette, query]);

  const projectRows = useMemo(() => {
    if (semanticMap.projects.length > 0) {
      return semanticMap.projects.map((row) => ({
        name: row.name,
        detail: `${row.node_count} nodes · ${row.edge_count} edges`,
      }));
    }
    if (projects.length > 0) {
      return projects.map((row) => ({
        name: row.name,
        detail: `${row.nodes} nodes · ${row.edges} edges`,
      }));
    }
    if (mockMode) {
      return MOCK_NODES.map((row) => ({
        name: row.name,
        detail: `${row.edges} edges · mock`,
      }));
    }
    return [];
  }, [mockMode, projects, semanticMap.projects]);

  const experienceRows = useMemo(() => {
    if (remoteExperiences.length > 0) {
      return remoteExperiences;
    }
    if (!isTauri()) {
      return mockSearchExperiences(query);
    }
    const q = query.trim().toLowerCase();
    return experiences
      .filter((row) =>
        !q
          ? true
          : `${row.project_id} ${row.adr_summary} ${row.agent}`
              .toLowerCase()
              .includes(q),
      )
      .slice(0, 8);
  }, [experiences, query, remoteExperiences]);

  const runGrokTest = useCallback(async () => {
    if (grokBusy) {
      return;
    }
    setGrokBusy(true);
    setStatus(null);
    try {
      if (!isTauri()) {
        setStatus("Grok Test yalnızca Tauri oturumunda — browser mock'ta NATS yok");
        return;
      }
      const result = await invoke<GrokTestResult>("trigger_grok_test", {
        projectId: selectedProject,
      });
      setStatus(`Grok Test → ${result.target_agent} · ${result.chain_label}`);
      close();
      router.push("/stream");
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      setStatus(text || "Grok Bot yok — tetiklenemedi");
    } finally {
      setGrokBusy(false);
    }
  }, [close, grokBusy, router, selectedProject]);

  const items = useMemo<PaletteItem[]>(() => {
    const q = query.trim();
    const out: PaletteItem[] = [];

    out.push({
      id: "action:grok-test",
      group: "Actions",
      title: "Trigger Grok Test",
      subtitle: selectedProject
        ? `lounge.task.requested → grok_bot · ${selectedProject}`
        : "lounge.task.requested → grok_bot",
      disabled: mockMode || grokBusy,
      hint: mockMode ? "Tauri only" : grokBusy ? "…" : "↵",
      run: () => void runGrokTest(),
    });

    for (const project of projectRows) {
      if (!matchesQuery(`${project.name} ${project.detail}`, q)) {
        continue;
      }
      out.push({
        id: `project:${project.name}`,
        group: "Switch Project",
        title: project.name,
        subtitle: project.detail,
        hint: selectedProject === project.name ? "active" : undefined,
        run: () => {
          switchProject(project.name);
          close();
          router.push("/vault");
        },
      });
    }

    for (const exp of experienceRows) {
      out.push({
        id: `exp:${exp.id}`,
        group: mockMode ? "Search Experience · mock" : "Search Experience",
        title: exp.adr_summary,
        subtitle: `${exp.project_id} · ${exp.agent} · ${exp.outcome}`,
        run: () => {
          focusExperience(exp);
          close();
          router.push("/vault");
        },
      });
    }

    const nodeSource =
      remoteNodes.length > 0
        ? remoteNodes
        : mockMode
          ? mockSearchNodes(q)
          : [];
    for (const node of nodeSource) {
      if (!matchesQuery(`${node.name} ${node.file ?? ""} ${node.kind}`, q)) {
        continue;
      }
      out.push({
        id: `node:${node.id || node.name}`,
        group: mockMode ? "Semantic nodes · mock" : "Semantic nodes",
        title: node.name,
        subtitle: [node.kind, node.file].filter(Boolean).join(" · ") || "memory_bridge",
        run: () => {
          if (node.kind === "project") {
            switchProject(node.name);
          } else {
            setVaultQuery(node.name);
          }
          close();
          router.push("/vault");
        },
      });
    }

    return out;
  }, [
    close,
    experienceRows,
    focusExperience,
    grokBusy,
    mockMode,
    projectRows,
    query,
    remoteNodes,
    router,
    runGrokTest,
    selectedProject,
    setVaultQuery,
    switchProject,
  ]);

  const selectActive = useCallback(() => {
    const item = items[active];
    if (!item || item.disabled) {
      return;
    }
    void item.run();
  }, [active, items]);

  const onInputKey = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((index) => Math.min(index + 1, Math.max(items.length - 1, 0)));
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((index) => Math.max(index - 1, 0));
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      selectActive();
    }
  };

  if (!openCommandPalette) {
    return null;
  }

  let lastGroup = "";

  return (
    <div
      className="fixed inset-0 z-[70] flex items-start justify-center bg-surface-container-lowest/70 px-4 pt-[12vh] backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-label="Command Palette"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          close();
        }
      }}
    >
      <div className="flex w-full max-w-xl flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container shadow-[0_24px_80px_rgba(0,0,0,0.55)]">
        <div className="flex items-center gap-2 border-b border-outline-variant bg-surface-container-low px-3 py-2.5">
          <span className="font-mono text-meta tracking-wider text-primary uppercase">⌘K</span>
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActive(0);
            }}
            onKeyDown={onInputKey}
            placeholder="Switch project · search experience · trigger grok…"
            className="min-w-0 flex-1 bg-transparent font-mono text-sm text-on-surface placeholder:text-outline focus:outline-none"
            aria-autocomplete="list"
            aria-controls="command-palette-list"
          />
          {searching ? (
            <span className="font-mono text-meta text-outline">…</span>
          ) : mockMode ? (
            <span className="rounded border border-outline-variant px-1.5 py-0.5 font-mono text-meta text-on-surface-variant uppercase">
              mock
            </span>
          ) : (
            <span className="font-mono text-meta text-outline">sqlite</span>
          )}
          <kbd className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-meta text-on-surface-variant">
            esc
          </kbd>
        </div>

        <div
          id="command-palette-list"
          role="listbox"
          className="max-h-[min(22rem,50vh)] overflow-auto py-1"
        >
          {items.length === 0 ? (
            <div className="px-3 py-6 text-center font-mono text-body text-on-surface-variant">
              Sonuç yok
            </div>
          ) : (
            items.map((item, index) => {
              const showGroup = item.group !== lastGroup;
              lastGroup = item.group;
              const selected = index === active;
              return (
                <div key={item.id}>
                  {showGroup ? (
                    <div className="px-3 pt-2 pb-1 font-mono text-meta tracking-wider text-outline uppercase">
                      {item.group}
                    </div>
                  ) : null}
                  <button
                    type="button"
                    role="option"
                    aria-selected={selected}
                    disabled={item.disabled}
                    onMouseEnter={() => setActive(index)}
                    onClick={() => {
                      if (!item.disabled) {
                        void item.run();
                      }
                    }}
                    className={`flex w-full items-start gap-2 px-3 py-2 text-left transition-colors ${
                      selected
                        ? "bg-primary-container/35 text-on-surface"
                        : "text-on-surface-variant hover:bg-surface-container-high"
                    } ${item.disabled ? "cursor-not-allowed opacity-50" : ""}`}
                  >
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-mono text-[12px] font-medium text-on-surface">
                        {item.title}
                      </div>
                      <div className="truncate font-mono text-meta text-outline">
                        {item.subtitle}
                      </div>
                    </div>
                    {item.hint ? (
                      <span className="shrink-0 font-mono text-meta text-primary uppercase">
                        {item.hint}
                      </span>
                    ) : null}
                  </button>
                </div>
              );
            })
          )}
        </div>

        {status ? (
          <div className="border-t border-outline-variant bg-surface-container-low px-3 py-2 font-mono text-meta text-error">
            {status}
          </div>
        ) : (
          <div className="border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-mono text-meta text-outline">
            ↑↓ seç · ↵ çalıştır · experience_store + memory_bridge
            {mockMode ? " · browser mock verisi" : ""}
          </div>
        )}
      </div>
    </div>
  );
}
