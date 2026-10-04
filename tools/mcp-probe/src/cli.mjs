import path from "node:path";
import { fileURLToPath } from "node:url";
import { runStdio } from "./stdio-transport.mjs";
import { startHttpServer } from "./http-transport.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const PACKAGE_ROOT = path.resolve(__dirname, "..");

/**
 * @param {string[]} argv
 */
export function parseArgs(argv) {
  /** @type {Record<string, string|boolean>} */
  const out = {
    transport: "stdio",
    host: "127.0.0.1",
    port: "19891",
    "log-dir": path.join(PACKAGE_ROOT, "logs"),
    "client-label": "",
    help: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--help" || a === "-h") {
      out.help = true;
      continue;
    }
    if (a.startsWith("--")) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (next && !next.startsWith("--")) {
        out[key] = next;
        i++;
      } else {
        out[key] = true;
      }
    }
  }
  return out;
}

export function printHelp(stream = process.stdout) {
  stream.write(`mcp-probe — independent MCP client-behavior probe

Usage:
  mcp-probe --transport stdio [--log-dir DIR] [--client-label NAME]
  mcp-probe --transport http [--host 127.0.0.1] [--port 19891] [--log-dir DIR]

Tools:
  ping
  slow_echo(delay_ms, message?, progress?)

Environment:
  MCP_PROBE_LOG_DIR     default log directory
  MCP_PROBE_CLIENT      default client label for log filenames

Docs: docs/qa/mcp-probe.md
`);
}

/**
 * @param {string[]} argv
 */
export async function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.help) {
    printHelp();
    return 0;
  }

  const logDir =
    process.env.MCP_PROBE_LOG_DIR ||
    /** @type {string} */ (args["log-dir"]);
  const clientLabel =
    /** @type {string} */ (args["client-label"]) ||
    process.env.MCP_PROBE_CLIENT ||
    "";
  const transport = String(args.transport || "stdio");

  if (transport === "stdio") {
    await runStdio({ logDir, clientLabel: clientLabel || "stdio" });
    return 0;
  }

  if (transport === "http") {
    const { url, port, close } = await startHttpServer({
      logDir,
      host: String(args.host),
      port: Number(args.port) || 19891,
      clientLabel: clientLabel || "http",
    });
    process.stderr.write(`[mcp-probe] Streamable HTTP listening at ${url}\n`);
    process.stderr.write(`[mcp-probe] health: http://${args.host}:${port}/health\n`);
    process.stderr.write(`[mcp-probe] logs: ${path.resolve(logDir)}\n`);

    const shutdown = async () => {
      await close();
      process.exit(0);
    };
    process.once("SIGINT", shutdown);
    process.once("SIGTERM", shutdown);
    // Keep alive
    await new Promise(() => {});
    return 0;
  }

  process.stderr.write(`Unknown transport: ${transport}\n`);
  printHelp(process.stderr);
  return 2;
}
