/** OS detection for shortcut labels and platform-specific UI copy. */

export type LoungePlatform = "macos" | "windows" | "linux";

function readUaPlatform(): string {
  if (typeof navigator === "undefined") {
    return "";
  }
  const uaData = (
    navigator as Navigator & {
      userAgentData?: { platform?: string };
    }
  ).userAgentData;
  if (uaData?.platform) {
    return uaData.platform;
  }
  return navigator.platform || navigator.userAgent || "";
}

/** Best-effort OS family for UI labels (browser + Tauri webview). */
export function detectPlatform(): LoungePlatform {
  const raw = readUaPlatform().toLowerCase();
  if (raw.includes("mac") || raw.includes("iphone") || raw.includes("ipad")) {
    return "macos";
  }
  if (raw.includes("win")) {
    return "windows";
  }
  return "linux";
}

/** Palette shortcut label: ⌘K on macOS, Ctrl+K elsewhere (§10.2 / SH-04b). */
export function paletteShortcutLabel(platform: LoungePlatform = detectPlatform()): string {
  return platform === "macos" ? "⌘K" : "Ctrl+K";
}

export function isApplePlatform(platform: LoungePlatform = detectPlatform()): boolean {
  return platform === "macos";
}
