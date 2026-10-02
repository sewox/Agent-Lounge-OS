import type { Page } from "@playwright/test";
import type { FixtureDataset } from "../fixtures/types";
import { FULL_FIXTURE } from "../fixtures/full";
import { EMPTY_FIXTURE } from "../fixtures/empty";

export type FixtureName = "full" | "empty" | "browser" | "reject";

export type QaDestructiveRow = {
  id: string;
  kind: string;
  command: string;
  pattern: string;
  source: string;
  class: string;
  command_hash: string;
};

declare global {
  interface Window {
    __QA_IPC_LOG__?: { cmd: string; args: unknown }[];
    __QA_FIXTURE__?: FixtureDataset;
    __QA_SEED_DESTRUCTIVE__?: (rows: QaDestructiveRow[]) => void;
    __TAURI_INTERNALS__?: Record<string, unknown>;
    __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
  }
}

/**
 * Inject window.__TAURI_INTERNALS__ invoke/event stubs before any app code runs.
 * Mocks live ONLY in the test harness — never in product source.
 */
export async function installTauriMock(page: Page, fixtureName: FixtureName = "full") {
  if (fixtureName === "browser") {
    // No __TAURI_INTERNALS__ — exercise browser / ?demo= paths.
    await page.addInitScript(() => {
      window.__QA_IPC_LOG__ = [];
    });
    return;
  }

  const fixture = fixtureName === "empty" || fixtureName === "reject" ? EMPTY_FIXTURE : FULL_FIXTURE;
  const rejectQuotaExperience = fixtureName === "reject";

  await page.addInitScript(({ data, rejectQuotaExperience }: { data: FixtureDataset; rejectQuotaExperience: boolean }) => {
    window.__QA_IPC_LOG__ = [];
    window.__QA_FIXTURE__ = data;
    (window as Window & { __QA_REJECT_QUOTA__?: boolean }).__QA_REJECT_QUOTA__ =
      rejectQuotaExperience;
    (window as Window & { __QA_IGNORED_SYMBOLS__?: FixtureDataset["deadSymbols"] }).__QA_IGNORED_SYMBOLS__ =
      (() => {
        try {
          const raw = sessionStorage.getItem("__QA_IGNORED_SYMBOLS__");
          return raw ? (JSON.parse(raw) as FixtureDataset["deadSymbols"]) : [];
        } catch {
          return [];
        }
      })();

    function persistIgnored(rows: FixtureDataset["deadSymbols"]) {
      (window as Window & { __QA_IGNORED_SYMBOLS__?: FixtureDataset["deadSymbols"] }).__QA_IGNORED_SYMBOLS__ =
        rows;
      try {
        sessionStorage.setItem("__QA_IGNORED_SYMBOLS__", JSON.stringify(rows));
      } catch {
        /* private mode */
      }
    }

    const qaSettings = {
      editor: {
        preset: "default" as string,
        custom_program: "",
        custom_args_template: "{path}",
      },
      ttlDays: 90,
      useCountThreshold: 0,
    };
    const qaDestructiveQueue: QaDestructiveRow[] = [];

    type ListenerMap = Map<string, number[]>;
    const listeners: ListenerMap = new Map();
    const callbacks = new Map<number, (data: unknown) => void>();

    function runCallback(id: number, payload: unknown) {
      const cb = callbacks.get(id);
      if (cb) cb(payload);
    }

    function emitPluginEvent(event: string, payload: unknown) {
      for (const handler of listeners.get(event) || []) {
        runCallback(handler, {
          event,
          id: Math.floor(Math.random() * 1e9),
          payload,
        });
      }
    }

    window.__QA_SEED_DESTRUCTIVE__ = (rows: QaDestructiveRow[]) => {
      qaDestructiveQueue.length = 0;
      for (const row of rows) {
        qaDestructiveQueue.push({ ...row, kind: row.kind || "destructive" });
      }
      const head = qaDestructiveQueue[0];
      if (head) {
        emitPluginEvent("approval_pending", {
          kind: "destructive",
          confirm_id: head.id,
          task_id: head.id,
        });
      }
    };

    function registerCallback(callback: (data: unknown) => void, once = false) {
      const id = window.crypto.getRandomValues(new Uint32Array(1))[0]!;
      callbacks.set(id, (payload) => {
        if (once) callbacks.delete(id);
        callback(payload);
      });
      return id;
    }

    function unregisterCallback(id: number) {
      callbacks.delete(id);
    }

    function seedBusEvents(handler: number) {
      const rows = window.__QA_FIXTURE__?.events ?? [];
      // Defer past LoungeProvider boot `setEvents([])` so SR-02 sees real traffic.
      window.setTimeout(() => {
        for (const ev of rows) {
          const kbMatch = String(ev.payload || "0.1kb").match(/([\d.]+)/);
          const payloadBytes = Math.max(1, Math.round(Number(kbMatch?.[1] || 0.1) * 1024));
          runCallback(handler, {
            event: "nats-event",
            id: Math.floor(Math.random() * 1e9),
            payload: {
              id: ev.id,
              type: "bus",
              subject: ev.subject,
              source_agent: ev.from,
              target_agent: ev.to,
              created_at: new Date().toISOString(),
              payload: {},
              payload_bytes: payloadBytes,
            },
          });
        }
      }, 120);
    }

    function handleEventPlugin(cmd: string, args: Record<string, unknown> | undefined) {
      if (cmd === "plugin:event|listen") {
        const event = String(args?.event ?? "");
        const handler = Number(args?.handler);
        if (!listeners.has(event)) listeners.set(event, []);
        listeners.get(event)!.push(handler);
        if (event === "nats-event") {
          seedBusEvents(handler);
        }
        return handler;
      }
      if (cmd === "plugin:event|emit") {
        const event = String(args?.event ?? "");
        for (const handler of listeners.get(event) || []) {
          runCallback(handler, { event, payload: args?.payload });
        }
        return null;
      }
      if (cmd === "plugin:event|unlisten") {
        return null;
      }
      return null;
    }

    async function invoke(cmd: string, args?: Record<string, unknown>) {
      window.__QA_IPC_LOG__!.push({ cmd, args: args ?? null });

      if (cmd.startsWith("plugin:event|")) {
        return handleEventPlugin(cmd, args);
      }

      const f = window.__QA_FIXTURE__!;
      const rejectLive = Boolean(
        (window as Window & { __QA_REJECT_QUOTA__?: boolean }).__QA_REJECT_QUOTA__,
      );
      switch (cmd) {
        case "list_experiences": {
          if (rejectLive) {
            throw new Error("list_experiences unavailable");
          }
          const limit = typeof args?.limit === "number" ? args.limit : 100;
          const offset = typeof args?.offset === "number" ? args.offset : 0;
          // Match Tauri 2 camelCase IPC args (includeArchived), not snake_case.
          const includeArchived = Boolean(args?.includeArchived);
          const rows = f.experiences
            .filter((row) => includeArchived || (row.status ?? "active") !== "archived")
            .slice()
            .sort((left, right) => {
              const pinDelta = Number(Boolean(right.is_pinned)) - Number(Boolean(left.is_pinned));
              if (pinDelta !== 0) {
                return pinDelta;
              }
              return right.created_at.localeCompare(left.created_at);
            });
          return rows.slice(offset, offset + limit);
        }
        case "count_experiences": {
          const includeArchived = Boolean(args?.includeArchived);
          return f.experiences.filter(
            (row) => includeArchived || (row.status ?? "active") !== "archived",
          ).length;
        }
        case "get_experience": {
          const id = String(args?.id ?? "");
          return f.experiences.find((row) => row.id === id) ?? null;
        }
        case "update_experience": {
          const id = String(args?.id ?? "");
          const patch = (args?.patch ?? {}) as Record<string, unknown>;
          const row = f.experiences.find((item) => item.id === id);
          if (!row) {
            throw new Error(`not found: ${id}`);
          }
          if (typeof patch.adr_summary === "string") {
            if (!row.original_content) {
              row.original_content = row.adr_summary;
            }
            row.adr_summary = patch.adr_summary;
          }
          if (typeof patch.project_id === "string") {
            row.project_id = patch.project_id;
          }
          if (typeof patch.outcome === "string") {
            row.outcome = patch.outcome as typeof row.outcome;
          }
          if (Array.isArray(patch.tags)) {
            row.tags = patch.tags.map(String);
          }
          row.updated_at = new Date().toISOString();
          return null;
        }
        case "archive_experience": {
          const id = String(args?.id ?? "");
          const row = f.experiences.find((item) => item.id === id);
          if (!row) {
            throw new Error(`not found: ${id}`);
          }
          row.status = "archived";
          row.archived_at = new Date().toISOString();
          row.archived_by = "user";
          return null;
        }
        case "unarchive_experience": {
          const id = String(args?.id ?? "");
          const row = f.experiences.find((item) => item.id === id);
          if (!row) {
            throw new Error(`not found: ${id}`);
          }
          row.status = "active";
          row.archived_at = null;
          row.archived_by = null;
          return null;
        }
        case "pin_experience": {
          const id = String(args?.id ?? "");
          const pinned = Boolean(args?.pinned);
          const row = f.experiences.find((item) => item.id === id);
          if (!row) {
            throw new Error(`not found: ${id}`);
          }
          row.is_pinned = pinned;
          return null;
        }
        case "mark_experience_reviewed": {
          const id = String(args?.id ?? "");
          const row = f.experiences.find((item) => item.id === id);
          if (!row) {
            throw new Error(`not found: ${id}`);
          }
          row.reviewed = true;
          return null;
        }
        case "mark_all_experiences_reviewed": {
          let changed = 0;
          for (const row of f.experiences) {
            if (row.reviewed === false && (row.status ?? "active") === "active") {
              row.reviewed = true;
              changed += 1;
            }
          }
          return changed;
        }
        case "count_unreviewed_experiences":
          return f.experiences.filter(
            (row) => row.reviewed === false && (row.status ?? "active") === "active",
          ).length;
        case "search_experiences": {
          if (rejectLive) {
            throw new Error("search_experiences unavailable");
          }
          const q = String(args?.query ?? args?.q ?? "").toLowerCase();
          if (!q) return f.experiences;
          return f.experiences.filter((row) =>
            `${row.adr_summary} ${row.project_id} ${row.agent} ${row.tags.join(" ")}`
              .toLowerCase()
              .includes(q),
          );
        }
        case "get_dead_symbols": {
          const ignored =
            (window as Window & { __QA_IGNORED_SYMBOLS__?: FixtureDataset["deadSymbols"] })
              .__QA_IGNORED_SYMBOLS__ ?? [];
          const ignoredKeys = new Set(
            ignored.map((row) =>
              [row.project_id ?? "", row.name, row.kind, row.file ?? "", String(row.line ?? "")].join("|"),
            ),
          );
          return f.deadSymbols.filter(
            (row) =>
              !ignoredKeys.has(
                [row.project_id ?? "", row.name, row.kind, row.file ?? "", String(row.line ?? "")].join("|"),
              ),
          );
        }
        case "list_ignored_symbols":
          return (
            (window as Window & { __QA_IGNORED_SYMBOLS__?: FixtureDataset["deadSymbols"] })
              .__QA_IGNORED_SYMBOLS__ ?? []
          );
        case "ignore_symbol": {
          const symbol = args?.symbol as FixtureDataset["deadSymbols"][number] | undefined;
          if (!symbol) return null;
          const store = [
            ...((window as Window & { __QA_IGNORED_SYMBOLS__?: FixtureDataset["deadSymbols"] })
              .__QA_IGNORED_SYMBOLS__ ?? []),
            symbol,
          ];
          persistIgnored(store);
          return null;
        }
        case "unignore_symbol": {
          const symbol = args?.symbol as FixtureDataset["deadSymbols"][number] | undefined;
          if (!symbol) return null;
          const store =
            (window as Window & { __QA_IGNORED_SYMBOLS__?: FixtureDataset["deadSymbols"] }).__QA_IGNORED_SYMBOLS__ ??
            [];
          persistIgnored(
            store.filter(
              (row) =>
                !(
                  row.name === symbol.name &&
                  row.project_id === symbol.project_id &&
                  row.file === symbol.file &&
                  row.kind === symbol.kind
                ),
            ),
          );
          return null;
        }
        case "open_dead_symbol_in_editor":
          return null;
        case "fix_dead_symbol_with_agent": {
          const hasAgent = f.workers.some((w) => w.is_active && w.enabled);
          if (!hasAgent) {
            throw new Error("No agent available — connect Grok Bot or another worker in Fleet");
          }
          const symbol = args?.symbol as FixtureDataset["deadSymbols"][number] | undefined;
          return {
            task_id: `fix-${Date.now()}`,
            subject: "lounge.task.requested",
            target_agent: "grok_bot",
            message: `Task queued for ${symbol?.name ?? "symbol"}`,
          };
        }
        case "get_semantic_map":
          return f.semanticMap;
        case "list_projects":
          return f.projects;
        case "ensure_services":
          return f.serviceReport;
        case "service_status":
          return f.serviceReport;
        case "get_runtime_paths":
          return {
            data_root: "/tmp/AgentLounge-qa",
            kernel_log: "/tmp/AgentLounge-qa/logs/kernel.log",
            lmr_log: "/tmp/AgentLounge-qa/data/lmr/serve.log",
            nats_log: "/tmp/AgentLounge-qa/data/nats/nats-server.log",
            lmr_dir: "/tmp/AgentLounge-qa/data/lmr",
            lmr_binary: "/tmp/AgentLounge-qa/data/lmr/ollama",
          };
        case "list_ollama_models":
          return f.models;
        case "get_kernel_model":
          return f.kernelModel;
        case "set_kernel_model": {
          const model = String(args?.model ?? "");
          f.kernelModel = model;
          return model;
        }
        case "get_quota_state":
          if (rejectLive) {
            throw new Error("get_quota_state unavailable");
          }
          return f.quotaState;
        case "list_quotas":
          if (rejectLive) {
            throw new Error("list_quotas unavailable");
          }
          return f.quotas;
        case "get_routing_policy":
          return f.policy;
        case "set_routing_policy": {
          f.policy = (args?.policy as typeof f.policy) ?? f.policy;
          return f.policy;
        }
        case "get_decision_gate_status":
          return f.decisionGate;
        case "enable_decision_gate":
          f.decisionGate = { ...f.decisionGate, phase: "ready", message: "Enabled" };
          return f.decisionGate;
        case "decline_decision_gate":
          f.decisionGate = { ...f.decisionGate, phase: "failed", message: "Declined" };
          return f.decisionGate;
        case "get_laya_engine_status":
        case "ensure_laya_engine":
          return f.layaEngine;
        case "pending_approvals":
          return f.pendingApprovals;
        case "focus_app_for_approval": {
          // Mirrors Rust focus_app_for_approval: emit approval_banner_focus so the
          // bridge can focus the banner (AP-10 desktop path in the harness).
          const taskId = String(args?.taskId ?? args?.task_id ?? "");
          const handlers = listeners.get("approval_banner_focus") || [];
          for (const handler of handlers) {
            runCallback(handler, {
              event: "approval_banner_focus",
              id: Math.floor(Math.random() * 1e9),
              payload: { task_id: taskId, reason: "notification_click" },
            });
          }
          // Also focus the banner node directly when listeners race (listen is async).
          const scoped = taskId
            ? document.querySelector<HTMLElement>(
                `[data-approval-chrome][data-task-id="${taskId}"] [data-qa="approval-banner"]`,
              )
            : null;
          const banner =
            scoped ?? document.querySelector<HTMLElement>('[data-qa="approval-banner"]');
          banner?.focus({ preventScroll: true });
          return null;
        }
        case "load_custom_approval_sound_data_url": {
          const name = String(args?.fileName ?? args?.file_name ?? "");
          if (!name || /[/:\\]/.test(name)) {
            throw new Error("invalid custom sound file name");
          }
          return `data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQAAAAA=`;
        }
        case "pick_custom_approval_sound": {
          const bytes = Number(args?.bytes ?? args?.size ?? 0);
          if (bytes > 5 * 1024 * 1024) {
            throw new Error("custom sound exceeds 5 MB limit");
          }
          return "custom-alert.wav";
        }
        case "list_pending_destructive":
          return qaDestructiveQueue.slice();
        case "confirm_destructive": {
          const id = String(args?.id ?? "");
          const hash = args?.commandHash != null ? String(args.commandHash) : "";
          const idx = qaDestructiveQueue.findIndex((row) => row.id === id);
          if (idx < 0) {
            throw new Error("unknown destructive confirmation id");
          }
          if (hash && qaDestructiveQueue[idx]!.command_hash !== hash) {
            throw new Error("destructive confirmation hash mismatch");
          }
          qaDestructiveQueue.splice(idx, 1);
          emitPluginEvent("approval_resolved", { task_id: id, reason: "confirmed" });
          return null;
        }
        case "reject_destructive": {
          const id = String(args?.id ?? "");
          const hash = args?.commandHash != null ? String(args.commandHash) : "";
          const idx = qaDestructiveQueue.findIndex((row) => row.id === id);
          if (idx < 0) {
            throw new Error("unknown destructive confirmation id");
          }
          if (hash && qaDestructiveQueue[idx]!.command_hash !== hash) {
            throw new Error("destructive confirmation hash mismatch");
          }
          qaDestructiveQueue.splice(idx, 1);
          emitPluginEvent("approval_resolved", { task_id: id, reason: "rejected" });
          return null;
        }
        case "get_editor_settings":
          return qaSettings.editor;
        case "set_editor_settings": {
          const next = (args?.settings ?? {}) as typeof qaSettings.editor;
          qaSettings.editor = {
            preset: String(next.preset ?? qaSettings.editor.preset),
            custom_program: String(next.custom_program ?? ""),
            custom_args_template: String(next.custom_args_template ?? "{path}"),
          };
          if (
            qaSettings.editor.preset === "custom" &&
            !qaSettings.editor.custom_args_template.includes("{path}")
          ) {
            throw new Error("editor argument template must include {path}");
          }
          return qaSettings.editor;
        }
        case "test_editor_settings":
          return null;
        case "get_experience_ttl_days":
          return qaSettings.ttlDays;
        case "set_experience_ttl_days": {
          const days = Number(args?.days ?? 90);
          if (days < 1 || days > 3650) {
            throw new Error("TTL must be between 1 and 3650 days");
          }
          qaSettings.ttlDays = days;
          return qaSettings.ttlDays;
        }
        case "get_experience_use_count_threshold":
          return qaSettings.useCountThreshold;
        case "set_experience_use_count_threshold": {
          qaSettings.useCountThreshold = Math.max(0, Number(args?.threshold ?? 0));
          return qaSettings.useCountThreshold;
        }
        case "resolve_routing": {
          const taskId = String(args?.taskId ?? "");
          f.pendingApprovals = f.pendingApprovals.filter((a) => a.task_id !== taskId);
          return null;
        }
        case "index_workspace": {
          const path = String(args?.path ?? "/tmp/workspace");
          const project = path.split(/[/\\]/).filter(Boolean).at(-1) || "workspace";
          return {
            project,
            status: "ok",
            nodes: f.semanticMap.projects[0]?.node_count ?? 0,
            edges: f.semanticMap.projects[0]?.edge_count ?? 0,
            files: f.semanticMap.projects[0]?.files ?? 0,
            dead: f.deadSymbols.length,
          };
        }
        case "scan_workspace": {
          const path = String(args?.path ?? "/tmp/workspace-scan");
          const discovered =
            f.projects.length > 0
              ? f.projects.map((p) => ({
                  name: p.name,
                  root_path: p.root_path ?? `${path}/${p.name}`,
                  nodes: 0,
                  edges: 0,
                  files: 0,
                }))
              : [
                  {
                    name: "alpha",
                    root_path: `${path}/alpha`,
                    nodes: 0,
                    edges: 0,
                    files: 0,
                  },
                  {
                    name: "beta",
                    root_path: `${path}/beta`,
                    nodes: 0,
                    edges: 0,
                    files: 0,
                  },
                ];
          const now = new Date().toISOString();
          const jobs = discovered.map((row, index) => ({
            id: `qa-job-${index + 1}`,
            project: row.name,
            repo_path: row.root_path,
            status: "queued",
            error: null,
            error_code: null,
            snapshot: null,
            enqueued_at: now,
            updated_at: now,
          }));
          (window as Window & { __QA_INDEX_JOBS__?: typeof jobs }).__QA_INDEX_JOBS__ = jobs;
          // Import into fixture projects immediately (registration).
          f.projects = discovered.map((row) => ({
            name: row.name,
            root_path: row.root_path,
            nodes: 0,
            edges: 0,
            files: 0,
          }));
          // Background indexing simulation — independent of invoke lifecycle.
          window.setTimeout(() => {
            const done = jobs.map((job) => ({
              ...job,
              status: "done" as const,
              snapshot: {
                project: job.project,
                status: "ok",
                nodes: 12,
                edges: 4,
                files: 6,
                dead: 0,
              },
              updated_at: new Date().toISOString(),
            }));
            (window as Window & { __QA_INDEX_JOBS__?: typeof done }).__QA_INDEX_JOBS__ = done;
            f.projects = done.map((job) => ({
              name: job.project,
              root_path: job.repo_path,
              nodes: 12,
              edges: 4,
              files: 6,
            }));
            f.semanticMap = {
              projects: done.map((job) => ({
                name: job.project,
                repo_path: job.repo_path,
                files: 6,
                node_count: 12,
                edge_count: 4,
                nodes: [],
                references: [],
                dead: [],
              })),
            };
            const handlers = listeners.get("lounge://index-job") || [];
            for (const job of done) {
              for (const handler of handlers) {
                runCallback(handler, {
                  event: "lounge://index-job",
                  payload: {
                    job,
                    progress: {
                      total: done.length,
                      queued: 0,
                      indexing: 0,
                      done: done.length,
                      failed: 0,
                      cancelled: 0,
                    },
                    jobs: done,
                  },
                });
              }
            }
          }, 250);
          return {
            workspace_path: path,
            discovered,
            jobs,
            progress: {
              total: jobs.length,
              queued: jobs.length,
              indexing: 0,
              done: 0,
              failed: 0,
              cancelled: 0,
            },
          };
        }
        case "list_index_jobs":
          return (window as Window & { __QA_INDEX_JOBS__?: unknown[] }).__QA_INDEX_JOBS__ ?? [];
        case "cancel_index_job":
        case "cancel_all_index_jobs":
          return true;
        case "probe_bus":
          return {
            id: `probe-${Date.now()}`,
            type: "probe",
            subject: "lounge.bus.probe",
            source_agent: "ui",
            created_at: new Date().toISOString(),
            payload: { ok: true },
            payload_bytes: 12,
          };
        case "record_whisper_feedback":
          return null;
        case "search_index_nodes": {
          const q = String(args?.query ?? args?.q ?? "").toLowerCase();
          const nodes = f.semanticMap.projects.flatMap((p) => p.nodes);
          if (!q) return nodes;
          return nodes.filter((n) => n.name.toLowerCase().includes(q));
        }
        case "trigger_grok_test":
          return { ok: true, subject: "lounge.task.requested" };
        case "get_graph_ui_port":
          return f.graphUiPort;
        case "set_graph_ui_port": {
          f.graphUiPort = Number(args?.port ?? f.graphUiPort);
          return f.graphUiPort;
        }
        case "get_graph_ui_status":
          return f.graphUiStatus;
        case "enable_graph_ui_cmd":
          f.graphUiStatus = { ...f.graphUiStatus, enabled: true };
          return null;
        case "open_graph_ui":
          f.graphUiStatus = { ...f.graphUiStatus, running: true, url: `http://127.0.0.1:${f.graphUiPort}` };
          return null;
        case "get_discovery_report": {
          const d = f.discovery;
          return {
            ...d,
            apps: d.apps ?? [],
            models: d.models ?? [],
            mcp_servers: d.mcp_servers ?? [],
            tools: Array.isArray(d.tools) ? d.tools : [],
            sources: Array.isArray(d.sources) ? d.sources : [],
            system_tools: Array.isArray(d.system_tools) ? d.system_tools : [],
          };
        }
        case "list_connected_tools":
          return f.connectedTools;
        case "list_recommended_models":
          return {
            device: {
              total_ram_gb: 16,
              available_ram_gb: 10,
              usable_budget_gb: 7.2,
              arch: "aarch64",
              apple_silicon: true,
              metal: true,
              summary: "16 GB · fixture device",
              recommended_upper: "8B Q4",
            },
            offers: [],
          };
        case "save_selected_tools":
          return args?.tools ?? f.connectedTools;
        case "pull_lmr_model":
          return String(args?.hfId ?? "model");
        case "list_workers":
        case "list_nats_workers":
          return f.workers;
        case "agent_efficiency_report":
          return {
            generated_at: new Date().toISOString(),
            window: "weekly",
            project_id: args?.projectId ?? null,
            markdown: "# Efficiency\n\nfixture report\n",
            crossProjectExperienceHits: f.experiences.length,
            tasks: 3,
            successes: 2,
          };
        // PR-4+ stubs — returning errors surfaces missing UX.
        case "delete_experience":
        case "approve_experience":
        case "open_in_editor":
          return null;
        default:
          console.warn(`[qa-tauri-mock] unhandled invoke: ${cmd}`, args);
          return null;
      }
    }

    window.__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: registerCallback,
      unregisterCallback,
      runCallback,
      callbacks,
      convertFileSrc: (filePath: string, protocol = "asset") =>
        `${protocol}://localhost/${encodeURIComponent(filePath)}`,
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { windowLabel: "main", label: "main" },
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };
  }, { data: fixture, rejectQuotaExperience });
}

export async function getIpcLog(page: Page) {
  return page.evaluate(() => window.__QA_IPC_LOG__ ?? []);
}

/** Seed the mock destructive FIFO queue and notify the app (Tauri mock only). */
export async function seedDestructiveQueue(page: Page, rows: QaDestructiveRow[]) {
  await page.evaluate((seed) => {
    const fn = window.__QA_SEED_DESTRUCTIVE__;
    if (!fn) {
      throw new Error("__QA_SEED_DESTRUCTIVE__ missing — use a Tauri mock fixture, not browser");
    }
    fn(seed);
  }, rows);
}

export async function waitForAppReady(page: Page) {
  await page.waitForSelector("main", { timeout: 30_000 });
  // Allow LoungeProvider boot (setTimeout 0 + refresh invokes) and fixture bus seed.
  await page.waitForTimeout(550);
}
