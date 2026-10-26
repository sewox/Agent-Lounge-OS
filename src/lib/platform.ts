/** OS detection for shortcut labels and platform-specific UI copy. */

export type LoungePlatform = "macos" | "windows" | "linux" | "unknown";

/**
 * SSR / first-hydration platform snapshot.
 * Non-macOS default keeps shortcut labels stable (Ctrl+K) until client mount.
 */
export const SSR_DEFAULT_PLATFORM: LoungePlatform = "linux";

/** Pure-helper default when caller has not resolved a real OS yet (SSR-safe). */
export const UNKNOWN_PLATFORM: LoungePlatform = "unknown";

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

/**
 * Best-effort OS family for UI labels (browser + Tauri webview).
 * Unknown platforms (ChromeOS, Android, other) fall back to `"linux"` —
 * fine for shortcut/label copy; not used for packaging or path semantics.
 */
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
export function paletteShortcutLabel(platform: LoungePlatform): string {
  return platform === "macos" ? "⌘K" : "Ctrl+K";
}

export function isApplePlatform(platform: LoungePlatform): boolean {
  return platform === "macos";
}

/**
 * Project-ID-Core identity equality: case-insensitive on macOS/Windows,
 * case-sensitive on Linux / unknown (never substring).
 */
export function projectIdentityEquals(
  left: string,
  right: string,
  platform: LoungePlatform = UNKNOWN_PLATFORM,
): boolean {
  if (platform === "macos" || platform === "windows") {
    return left.localeCompare(right, undefined, { sensitivity: "accent" }) === 0;
  }
  return left === right;
}
