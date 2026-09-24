"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { isTauri, type ConnectedTool } from "@/lib/lounge";

export default function Home() {
  const router = useRouter();
  useEffect(() => {
    const id = window.setTimeout(() => {
      if (!isTauri()) {
        router.replace("/onboarding");
        return;
      }
      void invoke<ConnectedTool[]>("list_connected_tools")
        .then((tools) => {
          const hasEnabled = tools.some((tool) => tool.enabled || tool.is_active);
          router.replace(hasEnabled ? "/dashboard" : "/onboarding");
        })
        .catch(() => {
          router.replace("/onboarding");
        });
    }, 0);
    return () => window.clearTimeout(id);
  }, [router]);
  return null;
}
