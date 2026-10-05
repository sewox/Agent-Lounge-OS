"use client";

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "@/lib/lounge";
import {
  buildMcpJson,
  demoRemoteAccess,
  type RemoteAccessInfo,
} from "@/lib/remote-access";

export function RemoteAccessCard() {
  const { t } = useTranslation("fleet");
  const [tunnelUrl, setTunnelUrl] = useState("https://your-tunnel.example");
  const [info, setInfo] = useState<RemoteAccessInfo>(() => demoRemoteAccess());
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async (url: string) => {
    if (!isTauri()) {
      setInfo(demoRemoteAccess(url));
      return;
    }
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const next = await invoke<RemoteAccessInfo>("get_remote_access", {
        tunnelUrl: url,
      });
      setInfo(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setInfo(demoRemoteAccess(url));
    }
  }, []);

  useEffect(() => {
    void refresh(tunnelUrl);
  }, [refresh, tunnelUrl]);

  const mcpJson =
    info.mcp_json?.trim() ||
    buildMcpJson(tunnelUrl || info.mcp_url, info.lounge_token);

  const onRegister = async () => {
    if (!isTauri()) {
      const next = demoRemoteAccess(tunnelUrl);
      next.allowed_hosts = [
        ...new Set([
          ...next.allowed_hosts,
          tunnelUrl.replace(/^https?:\/\//, "").split("/")[0] || "",
        ]),
      ].filter(Boolean);
      next.mcp_json = buildMcpJson(tunnelUrl, next.lounge_token);
      setInfo(next);
      return;
    }
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const next = await invoke<RemoteAccessInfo>("register_remote_tunnel", {
        tunnelUrl,
      });
      setInfo(next);
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
          {t("remoteAccessTokenLabel")}: {info.lounge_token}
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
      {info.allowed_hosts.length > 0 ? (
        <p className="mt-2 font-mono text-meta text-outline">
          {t("remoteAccessAllowed")}: {info.allowed_hosts.join(", ")}
        </p>
      ) : null}
    </section>
  );
}
