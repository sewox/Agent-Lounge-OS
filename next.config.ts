import type { NextConfig } from "next";
import {
  resolveDevAssetPrefix,
  resolveDevPort,
} from "./src/lib/dev-asset-prefix";

const isProd = process.env.NODE_ENV === "production";
const internalHost = process.env.TAURI_DEV_HOST || "localhost";

const nextConfig: NextConfig = {
  output: "export",
  images: {
    unoptimized: true,
  },
  // Hide the floating Next.js dev indicator so it never overlays bottom-left UI.
  devIndicators: false,
  // Must track PORT (default 3000) — same source as scripts/next-dev.mjs.
  assetPrefix: resolveDevAssetPrefix({
    isProd,
    host: internalHost,
    port: resolveDevPort(),
  }),
};

export default nextConfig;
