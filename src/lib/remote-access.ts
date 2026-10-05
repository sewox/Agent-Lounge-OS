/** Client-side helpers for Connect Grok Bot / Managed Origin Validation (PR-5). */

export type RemoteAccessInfo = {
  auth_required: boolean;
  nats_auth_active: boolean;
  lounge_token: string;
  mcp_bind: string;
  mcp_url: string;
  allowed_hosts: string[];
  tunnel_url: string | null;
  mcp_json: string;
  nats_creds_file: string;
};

export function buildMcpJson(mcpUrl: string, token: string): string {
  const url = mcpUrl.trim().replace(/\/+$/, "");
  const endpoint = url.endsWith("/mcp") ? url : `${url}/mcp`;
  return `${JSON.stringify(
    {
      mcpServers: {
        "agent-lounge-os": {
          url: endpoint,
          headers: {
            "X-Lounge-Token": token,
          },
        },
      },
    },
    null,
    2,
  )}\n`;
}

export function demoRemoteAccess(tunnelUrl = "https://your-tunnel.example"): RemoteAccessInfo {
  const token = "lounge_demo_token";
  const mcp_json = buildMcpJson(tunnelUrl, token);
  return {
    auth_required: true,
    nats_auth_active: false,
    lounge_token: token,
    mcp_bind: "127.0.0.1:18791",
    mcp_url: `${tunnelUrl.replace(/\/+$/, "")}/mcp`,
    allowed_hosts: [],
    tunnel_url: tunnelUrl,
    mcp_json,
    nats_creds_file: "",
  };
}
