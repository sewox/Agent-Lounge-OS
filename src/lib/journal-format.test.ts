import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";
import {
  formatJournalDurationMs,
  journalQueryFilterWire,
  queryEventJournal,
  shortJournalHash,
} from "./lounge.ts";

describe("journal duration / hash helpers", () => {
  it("formats EN and TR second suffixes", () => {
    assert.equal(formatJournalDurationMs(6000, "en"), "6.0s");
    assert.equal(formatJournalDurationMs(3500, "en-US"), "3.5s");
    assert.equal(formatJournalDurationMs(2500, "tr"), "2.5sn");
    assert.equal(formatJournalDurationMs(3500, "tr-TR"), "3.5sn");
    assert.equal(formatJournalDurationMs(null, "en"), "—");
  });

  it("shortens entry hashes", () => {
    assert.equal(shortJournalHash("abcdef0123456789"), "abcdef01");
    assert.equal(shortJournalHash("ab"), "ab");
    assert.equal(shortJournalHash(null), "—");
  });
});

describe("queryEventJournal nested filter wire (B1)", () => {
  const g = globalThis as typeof globalThis & {
    window?: Window & { __TAURI_INTERNALS__?: Record<string, unknown> };
  };
  const prevWindow = g.window;

  afterEach(() => {
    if (prevWindow === undefined) {
      delete g.window;
    } else {
      g.window = prevWindow;
    }
  });

  it("journalQueryFilterWire emits snake_case keys for Rust JournalQueryFilter", () => {
    const wire = journalQueryFilterWire({
      taskId: "t-1",
      projectId: "p-1",
      actionType: "task.requested",
      limit: 25,
    });
    assert.equal(wire.task_id, "t-1");
    assert.equal(wire.project_id, "p-1");
    assert.equal(wire.action_type, "task.requested");
    assert.equal(wire.limit, 25);
    assert.equal("taskId" in wire, false);
    assert.equal("projectId" in wire, false);
    assert.equal("actionType" in wire, false);
  });

  it("queryEventJournal invokes query_event_journal with snake_case filter fields", async () => {
    const calls: { cmd: string; args: Record<string, unknown> }[] = [];
    g.window = {
      __TAURI_INTERNALS__: {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          calls.push({ cmd, args: args ?? {} });
          return [];
        },
      },
    } as Window & { __TAURI_INTERNALS__: Record<string, unknown> };

    await queryEventJournal({
      taskId: "t-1",
      projectId: "p-1",
      actionType: "task.requested",
      limit: 25,
    });

    assert.equal(calls.length, 1);
    assert.equal(calls[0]?.cmd, "query_event_journal");
    const filter = calls[0]?.args.filter as Record<string, unknown>;
    assert.ok(filter && typeof filter === "object");
    assert.equal(filter.task_id, "t-1");
    assert.equal(filter.project_id, "p-1");
    assert.equal(filter.action_type, "task.requested");
    assert.equal(filter.limit, 25);
    assert.equal(filter.taskId, undefined);
    assert.equal(filter.projectId, undefined);
    assert.equal(filter.actionType, undefined);
  });
});
