import type { Page } from "@playwright/test";
import type { FixtureDataset } from "../fixtures/types";
import { FULL_FIXTURE } from "../fixtures/full";
import { EMPTY_FIXTURE } from "../fixtures/empty";

export type FixtureName = "full" | "empty" | "browser";

declare global {
  interface Window {
    __QA_IPC_LOG__?: { cmd: string; args: unknown }[];
    __QA_FIXTURE__?: FixtureDataset;
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

  const fixture = fixtureName === "empty" ? EMPTY_FIXTURE : FULL_FIXTURE;

  await page.addInitScript((data: FixtureDataset) => {
    window.__QA_IPC_LOG__ = [];
    window.__QA_FIXTURE__ = data;

    type ListenerMap = Map<string, number[]>;
    const listeners: ListenerMap = new Map();
    const callbacks = new Map<number, (data: unknown) => void>();

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

    function runCallback(id: number, payload: unknown) {
      const cb = callbacks.get(id);
      if (cb) cb(payload);
    }

    function handleEventPlugin(cmd: string, args: Record<string, unknown> | undefined) {
      if (cmd === "plugin:event|listen") {
        const event = String(args?.event ?? "");
        const handler = Number(args?.handler);
        if (!listeners.has(event)) listeners.set(event, []);
        listeners.get(event)!.push(handler);
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
      switch (cmd) {
        case "list_experiences": {
          const limit = typeof args?.limit === "number" ? args.limit : f.experiences.length;
          return f.experiences.slice(0, limit);
        }
        case "search_experiences": {
          const q = String(args?.query ?? args?.q ?? "").toLowerCase();
          if (!q) return f.experiences;
          return f.experiences.filter((row) =>
            `${row.adr_summary} ${row.project_id} ${row.agent} ${row.tags.join(" ")}`
              .toLowerCase()
              .includes(q),
          );
        }
        case "get_dead_symbols":
          return f.deadSymbols;
        case "get_semantic_map":
          return f.semanticMap;
        case "list_projects":
          return f.projects;
        case "ensure_services":
          return f.serviceReport;
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
          return f.quotaState;
        case "list_quotas":
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
        case "get_discovery_report":
          return f.discovery;
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
              summary: "fixture device",
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
        // PR-1 stubs — expected to be missing today; returning errors surfaces missing UX.
        case "get_experience":
        case "update_experience":
        case "delete_experience":
        case "archive_experience":
        case "pin_experience":
        case "approve_experience":
        case "ignore_dead_symbol":
        case "open_in_editor":
        case "create_task_from_dead_symbol":
          throw new Error(`command not found: ${cmd}`);
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
  }, fixture);
}

export async function getIpcLog(page: Page) {
  return page.evaluate(() => window.__QA_IPC_LOG__ ?? []);
}

export async function waitForAppReady(page: Page) {
  await page.waitForSelector("main", { timeout: 30_000 });
  // Allow LoungeProvider boot (setTimeout 0 + refresh invokes).
  await page.waitForTimeout(400);
}
