import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  eventSortKey,
  sortEventsNewestFirst,
  type NatsEvent,
} from "./lounge.ts";

function ev(
  partial: Pick<NatsEvent, "id" | "time"> & Partial<NatsEvent>,
): NatsEvent {
  return {
    subject: "lounge.task.requested",
    from: "kernel",
    to: "bus",
    payload: "0.1kb",
    state: "ok",
    ...partial,
  };
}

describe("sortEventsNewestFirst", () => {
  it("orders by clock time newest first when createdAtMs is absent", () => {
    // Mirrors live Linux dashboard disorder: 39.349, 38.691, 36.440, 36.785, …
    const shuffled = [
      ev({ id: "a", time: "21:41:36.440" }),
      ev({ id: "b", time: "21:41:39.349" }),
      ev({ id: "c", time: "21:41:36.785" }),
      ev({ id: "d", time: "21:41:38.691" }),
      ev({ id: "e", time: "21:41:36.120" }),
      ev({ id: "f", time: "21:41:37.998" }),
      ev({ id: "g", time: "21:41:37.999" }),
    ];
    const sorted = sortEventsNewestFirst(shuffled);
    assert.deepEqual(
      sorted.map((row) => row.time),
      [
        "21:41:39.349",
        "21:41:38.691",
        "21:41:37.999",
        "21:41:37.998",
        "21:41:36.785",
        "21:41:36.440",
        "21:41:36.120",
      ],
    );
  });

  it("prefers createdAtMs over display time (out-of-order arrival)", () => {
    const rows = [
      ev({ id: "late", time: "21:41:40.000", createdAtMs: 1_000 }),
      ev({ id: "early", time: "21:41:10.000", createdAtMs: 3_000 }),
      ev({ id: "mid", time: "21:41:50.000", createdAtMs: 2_000 }),
    ];
    assert.deepEqual(
      sortEventsNewestFirst(rows).map((row) => row.id),
      ["early", "mid", "late"],
    );
  });

  it("is stable for equal timestamps", () => {
    const rows = [
      ev({ id: "first", time: "21:41:36.440", createdAtMs: 500 }),
      ev({ id: "second", time: "21:41:36.440", createdAtMs: 500 }),
      ev({ id: "third", time: "21:41:36.440", createdAtMs: 500 }),
    ];
    assert.deepEqual(
      sortEventsNewestFirst(rows).map((row) => row.id),
      ["first", "second", "third"],
    );
  });

  it("eventSortKey parses HH:mm:ss.mmm", () => {
    assert.equal(eventSortKey({ time: "00:00:01.500" }), 1500);
    assert.ok(eventSortKey({ time: "21:41:39.349" }) > eventSortKey({ time: "21:41:38.691" }));
  });
});
