/** Pure helpers for Graph UI enable button — unit-tested without Tauri. */

export type GraphUiPortMode = "auto" | "user";

export type GraphUiButtonStatus = {
  port: number;
  port_conflict: boolean;
  conflict_message: string | null;
  info_message?: string | null;
  /** Preferred port skipped in Auto mode (busy/foreign). */
  remap_from_port?: number | null;
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
