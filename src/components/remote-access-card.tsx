"use client";

import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useIsTauri } from "@/hooks/use-is-tauri";
import {
  buildMcpJson,
  demoRemoteAccess,
  type RemoteAccessInfo,
} from "@/lib/remote-access";

/**
 * Connect Grok Bot helper — tunnel URL drives derived MCP JSON.
 * Tauri session (token / allow-list) loads once asynchronously; URL edits do not
 * setState from an effect (avoids react-hooks/set-state-in-effect).
 */
export function RemoteAccessCard() {
  const { t } = useTranslation("fleet");
  const tauriHost = useIsTauri();
  const [tunnelUrl, setTunnelUrl] = useState("https://your-tunnel.example");
  const [session, setSession] = useState<RemoteAccessInfo | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!tauriHost) {
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const next = await invoke<RemoteAccessInfo>("get_remote_access", {
          tunnelUrl: "https://your-tunnel.example",
        });
        if (!cancelled) {
          setSession(next);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tauriHost]);

  const demo = useMemo(() => demoRemoteAccess(tunnelUrl), [tunnelUrl]);
  const token = session?.lounge_token ?? demo.lounge_token;
  const allowedHosts = session?.allowed_hosts ?? demo.allowed_hosts;
  const mcpJson = useMemo(
    () => buildMcpJson(tunnelUrl, token),
    [tunnelUrl, token],
  );

  const onRegister = async () => {
    if (!tauriHost) {
      const host =
        tunnelUrl.replace(/^https?:\/\//, "").split("/")[0]?.toLowerCase() || "";
      setSession({
        ...demo,
        lounge_token: token,
        allowed_hosts: [...new Set([...allowedHosts, host].filter(Boolean))],
        tunnel_url: tunnelUrl,
        mcp_json: mcpJson,
        mcp_url: tunnelUrl.replace(/\/+$/, "").endsWith("/mcp")
          ? tunnelUrl.replace(/\/+$/, "")
          : `${tunnelUrl.replace(/\/+$/, "")}/mcp`,
      });
      return;
    }
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const next = await invoke<RemoteAccessInfo>("register_remote_tunnel", {
        tunnelUrl,
      });
      setSession(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const onCopy = async () => {
    try {
      await navigator.clipboard?.writeText(mcpJson);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  };

  return (
    <section
      data-qa="remote-access-card"
      className="shrink-0 border-t border-outline-variant bg-surface-container-low/50 px-3 py-3"
    >
      <div className="mb-2 flex flex-wrap items-end justify-between gap-2">
        <div>
          <h3 className="font-body text-meta font-semibold tracking-label text-on-surface uppercase">
            {t("remoteAccessTitle")}
          </h3>
          <p className="mt-1 max-w-2xl font-body text-body text-on-surface-variant">
            {t("remoteAccessBlurb")}
          </p>
        </div>
        <button
          type="button"
          data-qa="remote-access-copy"
          className="rounded border border-outline-variant bg-surface-container px-2.5 py-1.5 font-body text-meta font-medium text-on-surface hover:bg-surface-container-high"
          onClick={() => void onCopy()}
        >
          {copied ? t("remoteAccessCopied") : t("remoteAccessCopyJson")}
        </button>
      </div>
      <label className="mb-2 block font-body text-meta text-outline uppercase">
        {t("remoteAccessTunnelLabel")}
        <input
          data-qa="remote-access-tunnel"
          className="mt-1 w-full rounded border border-outline-variant bg-surface-container px-2 py-1.5 font-mono text-body text-on-surface"
          value={tunnelUrl}
          onChange={(event) => setTunnelUrl(event.target.value)}
          placeholder="https://….trycloudflare.com"
        />
      </label>
      <div className="mb-2 flex flex-wrap gap-2">
        <button
          type="button"
          data-qa="remote-access-register"
          className="rounded border border-secondary/40 bg-secondary-container/20 px-2.5 py-1.5 font-body text-meta font-medium text-secondary hover:bg-secondary-container/40"
          onClick={() => void onRegister()}
        >
          {t("remoteAccessAllowHost")}
        </button>
        <span className="font-mono text-meta text-on-surface-variant">
          {t("remoteAccessTokenLabel")}: {token}
        </span>
      </div>
      {error ? (
        <p className="mb-2 font-body text-meta text-error" data-qa="remote-access-error">
          {error}
        </p>
      ) : null}
      <pre
        data-qa="remote-access-mcp-json"
        className="max-h-48 overflow-auto rounded border border-outline-variant/60 bg-surface-container p-2 font-mono text-meta text-on-surface-variant whitespace-pre-wrap"
      >
        {mcpJson}
      </pre>
      {allowedHosts.length > 0 ? (
        <p className="mt-2 font-mono text-meta text-outline">
          {t("remoteAccessAllowed")}: {allowedHosts.join(", ")}
        </p>
      ) : null}
    </section>
  );
}
