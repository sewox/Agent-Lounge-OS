export const APPROVAL_SOUND_STORAGE_KEY = "lounge.approvalSound";

export const BUILTIN_SOUND_IDS = ["chime-soft", "chime-bright", "pulse-low", "pulse-high"] as const;

export type BuiltinSoundId = (typeof BUILTIN_SOUND_IDS)[number];

export type ApprovalSoundSettings = {
  enabled: boolean;
  soundId: BuiltinSoundId | "custom";
  customFileName: string | null;
  volume: number;
  intervalSecs: number;
};

export const DEFAULT_APPROVAL_SOUND_SETTINGS: ApprovalSoundSettings = {
  enabled: true,
  soundId: "chime-soft",
  customFileName: null,
  volume: 0.7,
  intervalSecs: 60,
};

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
    return {
      enabled: parsed.enabled ?? DEFAULT_APPROVAL_SOUND_SETTINGS.enabled,
      soundId:
        parsed.soundId === "custom" || isBuiltinSoundId(parsed.soundId)
          ? parsed.soundId
          : DEFAULT_APPROVAL_SOUND_SETTINGS.soundId,
      customFileName:
        typeof parsed.customFileName === "string" ? parsed.customFileName : null,
      volume: clampVolume(parsed.volume ?? DEFAULT_APPROVAL_SOUND_SETTINGS.volume),
      intervalSecs: clampIntervalSecs(
        parsed.intervalSecs ?? DEFAULT_APPROVAL_SOUND_SETTINGS.intervalSecs,
      ),
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
  try {
    store?.setItem(APPROVAL_SOUND_STORAGE_KEY, JSON.stringify(settings));
  } catch {
    /* ignore */
  }
}

export function builtinSoundSrc(id: BuiltinSoundId): string {
  return `/sounds/alerts/${id}.wav`;
}

export function resolveAlertSoundSrc(settings: ApprovalSoundSettings): string {
  if (settings.soundId === "custom" && settings.customFileName) {
    // Persisted value is a bare file name; runtime may also pass an asset:// URL.
    if (settings.customFileName.includes("/") || settings.customFileName.includes(":")) {
      return settings.customFileName;
    }
    return settings.customFileName;
  }
  const id = isBuiltinSoundId(settings.soundId) ? settings.soundId : "chime-soft";
  return builtinSoundSrc(id);
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
    this.play(settings);
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
    this.play(settings);
  }

  private play(settings: ApprovalSoundSettings): void {
    const audio = this.createAudio();
    audio.src = resolveAlertSoundSrc(settings);
    audio.volume = settings.volume;
    this.lastVolume = settings.volume;
    this.audio = audio;
    void audio.play().catch(() => {
      /* autoplay policy / missing asset */
    });
  }
}
