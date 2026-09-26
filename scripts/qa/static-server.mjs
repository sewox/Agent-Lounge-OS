#!/usr/bin/env node
/**
 * Static file server for Next `output: "export"` out/ directory.
 * Maps /vault → vault.html or vault/index.html.
 */
import http from "node:http";
import fs from "node:fs";
import path from "node:path";

const port = Number(process.env.PLAYWRIGHT_PORT || process.argv[2] || 4173);
const root = path.resolve(process.cwd(), process.env.PLAYWRIGHT_OUT_DIR || "out");

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".txt": "text/plain; charset=utf-8",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
};

function resolveFile(urlPath) {
  const clean = decodeURIComponent(urlPath.split("?")[0].split("#")[0]);
  const candidates = [];
  if (clean.endsWith("/")) {
    candidates.push(path.join(root, clean, "index.html"));
  } else {
    candidates.push(path.join(root, clean));
    candidates.push(path.join(root, `${clean}.html`));
    candidates.push(path.join(root, clean, "index.html"));
  }
  if (clean === "/" || clean === "") {
    candidates.unshift(path.join(root, "index.html"));
  }
  for (const c of candidates) {
    const resolved = path.resolve(c);
    if (!resolved.startsWith(root)) continue;
    if (fs.existsSync(resolved) && fs.statSync(resolved).isFile()) return resolved;
  }
  const fallback = path.join(root, "404.html");
  if (fs.existsSync(fallback)) return fallback;
  return null;
}

const server = http.createServer((req, res) => {
  const file = resolveFile(req.url || "/");
  if (!file) {
    res.writeHead(404, { "Content-Type": "text/plain" });
    res.end("Not found");
    return;
  }
  const ext = path.extname(file).toLowerCase();
  res.writeHead(200, { "Content-Type": MIME[ext] || "application/octet-stream" });
  fs.createReadStream(file).pipe(res);
});

server.listen(port, "127.0.0.1", () => {
  console.log(`qa-static serving ${root} on http://127.0.0.1:${port}`);
});
