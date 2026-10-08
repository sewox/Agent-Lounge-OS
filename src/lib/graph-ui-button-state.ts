/** Pure helpers for Graph UI enable button — unit-tested without Tauri. */

export type GraphUiPortMode = "auto" | "user";

export type GraphUiMessageParams = Record<string, string | number>;

export type GraphUiButtonStatus = {
  port: number;
  port_conflict: boolean;
  conflict_message: string | null;
  info_message?: string | null;
  /** Preferred port skipped in Auto mode (busy/foreign). */
  remap_from_port?: number | null;
  /** i18n key for conflict/info (frontend translates). */
  message_key?: string | null;
  message_params?: GraphUiMessageParams | null;
  port_mode: GraphUiPortMode;
  ui_available: boolean;
  project_indexed: boolean;
};

/** Disable Enable only while busy, or when User mode has a real conflict. */
export function isEnableGraphUiDisabled(
  busy: boolean,
  status: Pick<GraphUiButtonStatus, "port_conflict" | "port_mode">,
): boolean {
  return busy || (status.port_conflict && status.port_mode === "user");
}

/** Auto-mode early-return on conflict must not apply — only User mode blocks. */
export function shouldBlockEnableOnConflict(
  status: Pick<GraphUiButtonStatus, "port_conflict" | "port_mode">,
): boolean {
  return status.port_conflict && status.port_mode === "user";
}

export function autoPortInfoPorts(
  status: Pick<GraphUiButtonStatus, "port_mode" | "port" | "remap_from_port">,
): { busy: number; next: number } | null {
  if (status.port_mode !== "auto") {
    return null;
  }
  if (status.remap_from_port != null && status.remap_from_port !== status.port) {
    return { busy: status.remap_from_port, next: status.port };
  }
  return null;
}

/** Resolve a localisable Graph UI status note from backend codes + params. */
export function resolveGraphUiStatusMessage(
  status: Pick<
    GraphUiButtonStatus,
    "message_key" | "message_params" | "conflict_message" | "info_message" | "remap_from_port" | "port"
  >,
  translate: (key: string, params?: GraphUiMessageParams) => string,
): string | null {
  if (status.message_key) {
    const params: GraphUiMessageParams = { ...(status.message_params ?? {}) };
    if (status.remap_from_port != null && params.busy == null) {
      params.busy = status.remap_from_port;
    }
    if (params.next == null) {
      params.next = status.port;
    }
    if (params.port == null) {
      params.port = status.port;
    }
    return translate(status.message_key, params);
  }
  const fallback = status.conflict_message || status.info_message;
  return fallback?.trim() || null;
}
