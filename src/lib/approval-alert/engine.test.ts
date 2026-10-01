import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  ApprovalAlertEngine,
  BUILTIN_SOUND_IDS,
  clampIntervalSecs,
  clampVolume,
  clearCustomSoundDataUrlCache,
  DEFAULT_APPROVAL_SOUND_SETTINGS,
  parseApprovalSoundSettings,
  resolveAlertSoundSrc,
  resolveAlertSoundSrcAsync,
  sanitizeCustomSoundFileName,
  writeApprovalSoundSettings,
  type ApprovalSoundSettings,
} from "./settings.ts";

describe("approval sound settings", () => {
  it("ships exactly 4 built-in sounds", () => {
    assert.equal(BUILTIN_SOUND_IDS.length, 4);
  });

  it("clamps volume and interval", () => {
    assert.equal(clampVolume(1.5), 1);
    assert.equal(clampVolume(-1), 0);
    assert.equal(clampIntervalSecs(2), 5);
    assert.equal(clampIntervalSecs(900), 600);
  });

  it("parses persisted JSON", () => {
    const settings = parseApprovalSoundSettings(
      JSON.stringify({
        enabled: false,
        soundId: "pulse-high",
        volume: 0.4,
        intervalSecs: 30,
      }),
    );
    assert.equal(settings.enabled, false);
    assert.equal(settings.soundId, "pulse-high");
    assert.equal(settings.volume, 0.4);
    assert.equal(settings.intervalSecs, 30);
  });

  it("resolves bundled wav src", () => {
    assert.match(resolveAlertSoundSrc(DEFAULT_APPROVAL_SOUND_SETTINGS), /\.wav$/);
  });

  it("custom bare name is not a playable sync URL; async loader supplies data URL", async () => {
    clearCustomSoundDataUrlCache();
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      soundId: "custom",
      customFileName: "custom-alert.wav",
    };
    assert.equal(resolveAlertSoundSrc(settings), "");
    const src = await resolveAlertSoundSrcAsync(settings, async (name) => {
      assert.equal(name, "custom-alert.wav");
      return "data:audio/wav;base64,AAAA";
    });
    assert.equal(src, "data:audio/wav;base64,AAAA");
  });

  it("caches custom data URLs across resolve calls", async () => {
    clearCustomSoundDataUrlCache();
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      soundId: "custom",
      customFileName: "custom-alert.wav",
    };
    let loads = 0;
    const loader = async () => {
      loads += 1;
      return "data:audio/wav;base64,CACHED";
    };
    assert.equal(await resolveAlertSoundSrcAsync(settings, loader), "data:audio/wav;base64,CACHED");
    assert.equal(await resolveAlertSoundSrcAsync(settings, loader), "data:audio/wav;base64,CACHED");
    assert.equal(loads, 1);
  });

  it("writeApprovalSoundSettings never persists URLs", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => {
        store.set(key, value);
      },
    };
    writeApprovalSoundSettings(
      {
        ...DEFAULT_APPROVAL_SOUND_SETTINGS,
        soundId: "custom",
        customFileName: "asset://localhost/custom-alert.wav",
      },
      storage,
    );
    const raw = store.get("lounge.approvalSound") ?? "";
    assert.ok(!raw.includes("asset://"));
    assert.ok(!raw.includes("custom-alert.wav") || JSON.parse(raw).customFileName === null);
  });

  it("sanitize keeps bare names only", () => {
    assert.equal(sanitizeCustomSoundFileName("custom-alert.wav"), "custom-alert.wav");
    assert.equal(sanitizeCustomSoundFileName("data:audio/wav;base64,xx"), null);
    assert.equal(sanitizeCustomSoundFileName("https://evil/x.wav"), null);
    assert.equal(sanitizeCustomSoundFileName("../x.wav"), null);
  });
});

describe("ApprovalAlertEngine", () => {
  it("starts on pending, repeats at interval, stops on resolve, applies volume", () => {
    const plays: { src: string; volume: number; t: number }[] = [];
    let now = 0;
    const handles = new Map<number, () => void>();
    let nextHandle = 1;
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      volume: 0.55,
      intervalSecs: 10,
    };

    const engine = new ApprovalAlertEngine({
      now: () => now,
      readSettings: () => settings,
      createAudio: () => ({
        src: "",
        volume: 1,
        currentTime: 0,
        play: async function play(this: { src: string; volume: number }) {
          plays.push({ src: this.src, volume: this.volume, t: now });
        },
        pause() {
          /* noop */
        },
      }),
      timer: {
        setInterval(handler) {
          const id = nextHandle++;
          handles.set(id, handler);
          return id;
        },
        clearInterval(handle) {
          handles.delete(handle as number);
        },
      },
    });

    engine.onPending();
    assert.equal(plays.length, 1);
    assert.equal(plays[0]?.volume, 0.55);
    assert.ok(plays[0]?.src.includes("chime-soft"));
    assert.equal(engine.appliedVolume, 0.55);
    assert.equal(engine.appliedIntervalMs, 10_000);

    now += 10_000;
    for (const tick of handles.values()) {
      tick();
    }
    assert.equal(plays.length, 2);
    assert.equal(plays[1]?.volume, 0.55);

    engine.onResolved();
    assert.equal(handles.size, 0);
    const after = plays.length;
    for (const tick of handles.values()) {
      tick();
    }
    assert.equal(plays.length, after);
    assert.equal(engine.isPending, false);
  });

  it("plays custom sound via data URL from loader (Audio.src)", async () => {
    clearCustomSoundDataUrlCache();
    const dataUrl = "data:audio/wav;base64,Q3VzdG9tQWxlcnQ=";
    const plays: { src: string }[] = [];
    let loadCount = 0;
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      soundId: "custom",
      customFileName: "custom-alert.wav",
      intervalSecs: 5,
    };
    const handles = new Map<number, () => void>();
    let nextHandle = 1;

    const engine = new ApprovalAlertEngine({
      readSettings: () => settings,
      loadCustomSound: async (name) => {
        loadCount += 1;
        assert.equal(name, "custom-alert.wav");
        return dataUrl;
      },
      createAudio: () => ({
        src: "",
        volume: 1,
        currentTime: 0,
        play: async function play(this: { src: string }) {
          plays.push({ src: this.src });
        },
        pause() {},
      }),
      timer: {
        setInterval(handler) {
          const id = nextHandle++;
          handles.set(id, handler);
          return id;
        },
        clearInterval(handle) {
          handles.delete(handle as number);
        },
      },
    });

    engine.onPending();
    await new Promise((r) => setTimeout(r, 20));
    assert.equal(plays.length, 1);
    assert.equal(plays[0]?.src, dataUrl);

    for (const tick of handles.values()) {
      tick();
    }
    await new Promise((r) => setTimeout(r, 20));
    assert.equal(plays.length, 2);
    assert.equal(plays[1]?.src, dataUrl);
    // Second play should hit the cache (one disk/load).
    assert.equal(loadCount, 1);

    engine.onResolved();
    const after = plays.length;
    await new Promise((r) => setTimeout(r, 20));
    assert.equal(plays.length, after);
  });

  it("does not play custom sound if resolved before loader finishes", async () => {
    clearCustomSoundDataUrlCache();
    const plays: string[] = [];
    let resolveLoad!: (url: string) => void;
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      soundId: "custom",
      customFileName: "custom-alert.wav",
    };
    const engine = new ApprovalAlertEngine({
      readSettings: () => settings,
      loadCustomSound: () =>
        new Promise((resolve) => {
          resolveLoad = resolve;
        }),
      createAudio: () => ({
        src: "",
        volume: 1,
        currentTime: 0,
        play: async function play(this: { src: string }) {
          plays.push(this.src);
        },
        pause() {},
      }),
      timer: {
        setInterval() {
          return 1;
        },
        clearInterval() {},
      },
    });

    engine.onPending();
    engine.onResolved();
    resolveLoad("data:audio/wav;base64,LATE");
    await new Promise((r) => setTimeout(r, 20));
    assert.equal(plays.length, 0);
  });

  it("loader rejection does not throw", async () => {
    clearCustomSoundDataUrlCache();
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      soundId: "custom",
      customFileName: "custom-alert.wav",
    };
    const engine = new ApprovalAlertEngine({
      readSettings: () => settings,
      loadCustomSound: async () => {
        throw new Error("disk fail");
      },
      createAudio: () => ({
        src: "",
        volume: 1,
        currentTime: 0,
        play: async () => undefined,
        pause() {},
      }),
      timer: {
        setInterval() {
          return 1;
        },
        clearInterval() {},
      },
    });
    assert.doesNotThrow(() => engine.onPending());
    await new Promise((r) => setTimeout(r, 20));
    engine.onResolved();
  });

  it("refreshSettings restarts interval when intervalSecs changes", () => {
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      intervalSecs: 10,
    };
    const cleared: number[] = [];
    const started: number[] = [];
    let handleId = 1;
    const engine = new ApprovalAlertEngine({
      readSettings: () => settings,
      createAudio: () => ({
        src: "",
        volume: 1,
        currentTime: 0,
        play: async () => undefined,
        pause() {},
      }),
      timer: {
        setInterval(_handler, ms) {
          started.push(ms);
          return handleId++;
        },
        clearInterval(handle) {
          cleared.push(handle as number);
        },
      },
    });
    engine.onPending();
    assert.deepEqual(started, [10_000]);
    settings.intervalSecs = 20;
    engine.refreshSettings();
    assert.ok(cleared.length >= 1);
    assert.equal(started.at(-1), 20_000);
    engine.onResolved();
  });

  it("respects disabled settings", () => {
    const plays: number[] = [];
    const settings: ApprovalSoundSettings = {
      ...DEFAULT_APPROVAL_SOUND_SETTINGS,
      enabled: false,
    };
    const engine = new ApprovalAlertEngine({
      readSettings: () => settings,
      createAudio: () => ({
        src: "",
        volume: 1,
        currentTime: 0,
        play: async () => {
          plays.push(1);
        },
        pause() {},
      }),
      timer: {
        setInterval() {
          return 1;
        },
        clearInterval() {},
      },
    });
    engine.onPending();
    assert.equal(plays.length, 0);
  });
});
