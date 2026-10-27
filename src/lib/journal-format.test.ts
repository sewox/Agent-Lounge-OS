import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { formatJournalDurationMs, shortJournalHash } from "./lounge.ts";

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
