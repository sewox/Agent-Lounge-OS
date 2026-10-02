export const APPROVAL_SOUND_STORAGE_KEY = "lounge.approvalSound";

export const BUILTIN_SOUND_IDS = ["chime-soft", "chime-bright", "pulse-low", "pulse-high"] as const;

export type BuiltinSoundId = (typeof BUILTIN_SOUND_IDS)[number];

export type ApprovalSoundSettings = {
  enabled: boolean;
  soundId: BuiltinSoundId | "custom";
  customFileName: string | null;
  volume: number;
  intervalSecs: number;
  /** OS toast on pending approval (K1: no volume escalation when background). */
  osNotificationEnabled: boolean;
};

export const DEFAULT_APPROVAL_SOUND_SETTINGS: ApprovalSoundSettings = {
  enabled: true,
  soundId: "chime-soft",
  customFileName: null,
  volume: 0.7,
  intervalSecs: 60,
  osNotificationEnabled: true,
};

/** Cache data: URLs so repeats do not re-read/base64 up to 5 MB from disk. */
const customSoundDataUrlCache = new Map<string, string>();

export function clearCustomSoundDataUrlCache(): void {
  customSoundDataUrlCache.clear();
}

const MAX_INTERVAL_SECS = 600;
const MIN_INTERVAL_SECS = 5;

export function clampVolume(value: number): number {
  if (!Number.isFinite(value)) {
    return DEFAULT_APPROVAL_SOUND_SETTINGS.volume;
  }
  return Math.min(1, Math.max(0, value));
}

export function clampIntervalSecs(value: number): number {
  if (!Number.isFinite(value)) {
    return DEFAULT_APPROVAL_SOUND_SETTINGS.intervalSecs;
  }
  return Math.min(MAX_INTERVAL_SECS, Math.max(MIN_INTERVAL_SECS, Math.round(value)));
}

export function isBuiltinSoundId(value: unknown): value is BuiltinSoundId {
  return typeof value === "string" && (BUILTIN_SOUND_IDS as readonly string[]).includes(value);
}

export function parseApprovalSoundSettings(raw: string | null | undefined): ApprovalSoundSettings {
  if (!raw) {
    return { ...DEFAULT_APPROVAL_SOUND_SETTINGS };
  }
  try {
    const parsed = JSON.parse(raw) as Partial<ApprovalSoundSettings>;
    const customFileName = sanitizeCustomSoundFileName(
      typeof parsed.customFileName === "string" ? parsed.customFileName : null,
    );
    let soundId: ApprovalSoundSettings["soundId"] =
      parsed.soundId === "custom" || isBuiltinSoundId(parsed.soundId)
        ? parsed.soundId
        : DEFAULT_APPROVAL_SOUND_SETTINGS.soundId;
    if (soundId === "custom" && !customFileName) {
      soundId = DEFAULT_APPROVAL_SOUND_SETTINGS.soundId;
    }
    return {
      enabled: parsed.enabled ?? DEFAULT_APPROVAL_SOUND_SETTINGS.enabled,
      soundId,
      customFileName,
      volume: clampVolume(parsed.volume ?? DEFAULT_APPROVAL_SOUND_SETTINGS.volume),
      intervalSecs: clampIntervalSecs(
        parsed.intervalSecs ?? DEFAULT_APPROVAL_SOUND_SETTINGS.intervalSecs,
      ),
      osNotificationEnabled:
        parsed.osNotificationEnabled ?? DEFAULT_APPROVAL_SOUND_SETTINGS.osNotificationEnabled,
    };
  } catch {
    return { ...DEFAULT_APPROVAL_SOUND_SETTINGS };
  }
}

export function readApprovalSoundSettings(
  storage?: Pick<Storage, "getItem"> | null,
): ApprovalSoundSettings {
  const store =
    storage === undefined && typeof localStorage !== "undefined" ? localStorage : storage;
  try {
    const raw =
      store?.getItem(APPROVAL_SOUND_STORAGE_KEY) ??
      store?.getItem("approval-sound") ??
      null;
    return parseApprovalSoundSettings(raw);
  } catch {
    return { ...DEFAULT_APPROVAL_SOUND_SETTINGS };
  }
}

export function writeApprovalSoundSettings(
  settings: ApprovalSoundSettings,
  storage?: Pick<Storage, "setItem"> | null,
): void {
  const store =
    storage === undefined && typeof localStorage !== "undefined" ? localStorage : storage;
  const customFileName = sanitizeCustomSoundFileName(settings.customFileName);
  const payload: ApprovalSoundSettings = {
    ...settings,
    customFileName,
    soundId:
      settings.soundId === "custom" && !customFileName
        ? DEFAULT_APPROVAL_SOUND_SETTINGS.soundId
        : settings.soundId,
  };
  try {
    store?.setItem(APPROVAL_SOUND_STORAGE_KEY, JSON.stringify(payload));
  } catch {
    /* ignore */
  }
  // Same-tab listeners (storage events only fire cross-tab).
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event("lounge.approvalSound"));
  }
  // New custom file → drop cached data URL for that name.
  if (payload.soundId === "custom" && payload.customFileName) {
    customSoundDataUrlCache.delete(payload.customFileName);
  }
}

/** Bare file name only — never a path, URL, or data URI. */
export function isBareCustomSoundFileName(value: string | null | undefined): boolean {
  if (!value) {
    return false;
  }
  if (/[/:\\?#]/.test(value) || value.includes("..")) {
    return false;
  }
  return /\.(wav|mp3|ogg)$/i.test(value);
}

/** Strip accidental URLs/paths; persist only a safe basename under app data. */
export function sanitizeCustomSoundFileName(value: string | null | undefined): string | null {
  if (!value) {
    return null;
  }
  if (value.startsWith("data:") || value.includes("://")) {
    return null;
  }
  // Reject anything that looks like a path — callers must pass a bare file name.
  if (value.includes("/") || value.includes("\\") || value.includes("..")) {
    return null;
  }
  const base = value.trim();
  return isBareCustomSoundFileName(base) ? base : null;
}

export function builtinSoundSrc(id: BuiltinSoundId): string {
  return `/sounds/alerts/${id}.wav`;
}

/**
 * Sync resolver for bundled sounds (and in-memory data: URLs used in tests).
 * Custom bare names require [`resolveAlertSoundSrcAsync`] (Rust data URL).
 */
export function resolveAlertSoundSrc(settings: ApprovalSoundSettings): string {
  if (settings.soundId === "custom" && settings.customFileName) {
    if (settings.customFileName.startsWith("data:audio/")) {
      return settings.customFileName;
    }
    // Bare name is not a playable URL — async Rust load is required.
    return "";
  }
  const id = isBuiltinSoundId(settings.soundId) ? settings.soundId : "chime-soft";
  return builtinSoundSrc(id);
}

export type CustomSoundLoader = (fileName: string) => Promise<string>;

export async function resolveAlertSoundSrcAsync(
  settings: ApprovalSoundSettings,
  loadCustom?: CustomSoundLoader,
): Promise<string> {
  const sync = resolveAlertSoundSrc(settings);
  if (sync) {
    return sync;
  }
  const bare = sanitizeCustomSoundFileName(settings.customFileName);
  if (settings.soundId !== "custom" || !bare) {
    return builtinSoundSrc("chime-soft");
  }
  const cached = customSoundDataUrlCache.get(bare);
  if (cached) {
    return cached;
  }
  let url = "";
  try {
    if (loadCustom) {
      url = await loadCustom(bare);
    } else {
      const { invoke } = await import("@tauri-apps/api/core");
      url = await invoke<string>("load_custom_approval_sound_data_url", { fileName: bare });
    }
  } catch {
    return "";
  }
  if (url.startsWith("data:audio/")) {
    customSoundDataUrlCache.set(bare, url);
  }
  return url;
}

export type TimerLike = {
  setInterval: (handler: () => void, ms: number) => unknown;
  clearInterval: (handle: unknown) => void;
};

export type AudioFactory = () => Pick<
  HTMLAudioElement,
  "src" | "volume" | "play" | "pause" | "currentTime"
>;

export type ApprovalAlertEngineDeps = {
  timer?: TimerLike;
  createAudio?: AudioFactory;
  readSettings?: () => ApprovalSoundSettings;
  now?: () => number;
  loadCustomSound?: CustomSoundLoader;
};

/** Repeating HTML Audio alert while an approval is pending (AP-08 / AP-09 engine). */
export class ApprovalAlertEngine {
  private pending = false;
  private intervalHandle: unknown = null;
  private audio: ReturnType<AudioFactory> | null = null;
  private lastVolume = DEFAULT_APPROVAL_SOUND_SETTINGS.volume;
  private lastIntervalMs = DEFAULT_APPROVAL_SOUND_SETTINGS.intervalSecs * 1000;
  private readonly timer: TimerLike;
  private readonly createAudio: AudioFactory;
  private readonly readSettings: () => ApprovalSoundSettings;
  private readonly loadCustomSound?: CustomSoundLoader;

  constructor(deps: ApprovalAlertEngineDeps = {}) {
    this.timer = deps.timer ?? {
      setInterval: (handler, ms) => setInterval(handler, ms),
      clearInterval: (handle) => clearInterval(handle as ReturnType<typeof setInterval>),
    };
    this.createAudio =
      deps.createAudio ??
      (() => {
        if (typeof Audio === "undefined") {
          return {
            src: "",
            volume: 1,
            play: async () => undefined,
            pause: () => undefined,
            currentTime: 0,
          };
        }
        return new Audio();
      });
    this.readSettings = deps.readSettings ?? (() => readApprovalSoundSettings());
    this.loadCustomSound = deps.loadCustomSound;
  }

  get isPending(): boolean {
    return this.pending;
  }

  get appliedVolume(): number {
    return this.lastVolume;
  }

  get appliedIntervalMs(): number {
    return this.lastIntervalMs;
  }

  onPending(): void {
    this.pending = true;
    this.playOnce();
    this.ensureInterval();
  }

  onResolved(): void {
    this.pending = false;
    this.stopInterval();
    this.stopAudio();
  }

  refreshSettings(): void {
    if (!this.pending) {
      return;
    }
    this.ensureInterval();
  }

  preview(): void {
    const settings = this.readSettings();
    if (!settings.enabled) {
      return;
    }
    this.play(settings, true);
  }

  dispose(): void {
    this.onResolved();
  }

  private ensureInterval(): void {
    const settings = this.readSettings();
    const intervalMs = settings.intervalSecs * 1000;
    if (this.intervalHandle != null && intervalMs === this.lastIntervalMs) {
      return;
    }
    this.stopInterval();
    this.lastIntervalMs = intervalMs;
    if (!this.pending || !settings.enabled) {
      return;
    }
    this.intervalHandle = this.timer.setInterval(() => {
      if (this.pending) {
        this.playOnce();
      }
    }, intervalMs);
  }

  private stopInterval(): void {
    if (this.intervalHandle != null) {
      this.timer.clearInterval(this.intervalHandle);
      this.intervalHandle = null;
    }
  }

  private stopAudio(): void {
    if (this.audio) {
      this.audio.pause();
      this.audio.currentTime = 0;
      this.audio = null;
    }
  }

  private playOnce(): void {
    const settings = this.readSettings();
    if (!this.pending || !settings.enabled) {
      return;
    }
    this.play(settings, false);
  }

  private play(settings: ApprovalSoundSettings, allowWhenIdle: boolean): void {
    this.stopAudio();
    const audio = this.createAudio();
    audio.volume = settings.volume;
    this.lastVolume = settings.volume;
    this.audio = audio;

    const applySrc = (src: string) => {
      if (!src) {
        return;
      }
      if (this.audio !== audio) {
        return;
      }
      if (!allowWhenIdle && !this.pending) {
        return;
      }
      audio.src = src;
      void audio.play().catch(() => {
        /* autoplay policy / missing asset */
      });
    };

    const syncSrc = resolveAlertSoundSrc(settings);
    if (syncSrc) {
      applySrc(syncSrc);
      return;
    }

    void resolveAlertSoundSrcAsync(settings, this.loadCustomSound)
      .then(applySrc)
      .catch(() => {
        /* missing custom file */
      });
  }
}
