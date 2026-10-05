import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { buildMcpJson, demoRemoteAccess } from "./remote-access.ts";

describe("remote-access MCP JSON", () => {
  it("includes URL and X-Lounge-Token fields", () => {
    const json = buildMcpJson("https://abc.trycloudflare.com", "lounge_tok_1");
    const parsed = JSON.parse(json) as {
      mcpServers: { "agent-lounge-os": { url: string; headers: Record<string, string> } };
    };
    assert.equal(
      parsed.mcpServers["agent-lounge-os"].url,
      "https://abc.trycloudflare.com/mcp",
    );
    assert.equal(
      parsed.mcpServers["agent-lounge-os"].headers["X-Lounge-Token"],
      "lounge_tok_1",
    );
    assert.match(json, /X-Lounge-Token/);
    assert.match(json, /https:\/\/abc\.trycloudflare\.com\/mcp/);
  });

  it("demo helper is copy-ready", () => {
    const info = demoRemoteAccess("https://tunnel.example");
    assert.match(info.mcp_json, /tunnel\.example\/mcp/);
    assert.match(info.mcp_json, /lounge_demo_token/);
  });
});
