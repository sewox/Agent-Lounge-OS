import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { isHydrationConsoleMessage } from "@/lib/hydration-console";

describe("isHydrationConsoleMessage", () => {
  it("matches hydration wording and minified React #418 family", () => {
    assert.equal(
      isHydrationConsoleMessage("Warning: Text content did not match. Server: hydration"),
      true,
    );
    assert.equal(
      isHydrationConsoleMessage(
        "Error: Minified React error #418; visit https://react.dev/errors/418",
      ),
      true,
    );
    assert.equal(isHydrationConsoleMessage("Minified React error #425"), true);
    assert.equal(isHydrationConsoleMessage("Minified React error #400"), false);
    assert.equal(isHydrationConsoleMessage("network failed"), false);
  });
});
