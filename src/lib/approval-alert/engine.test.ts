import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  ApprovalAlertEngine,
  BUILTIN_SOUND_IDS,
  clampIntervalSecs,
  clampVolume,
  DEFAULT_APPROVAL_SOUND_SETTINGS,
  parseApprovalSoundSettings,
  resolveAlertSoundSrc,
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
