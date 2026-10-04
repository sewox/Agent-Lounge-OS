import readline from "node:readline";
import {
  createSession,
  handleMessage,
  onConnectionClose,
  parseRpc,
} from "./protocol.mjs";
import { JsonlLogger } from "./logger.mjs";

/**
 * Run MCP probe over stdio (newline-delimited JSON-RPC).
 * Long-running tools do not block the read loop so cancelled/EOF can be observed.
 * @param {object} opts
 * @param {string} opts.logDir
 * @param {string} [opts.clientLabel]
 * @param {NodeJS.ReadableStream} [opts.stdin]
 * @param {NodeJS.WritableStream} [opts.stdout]
 * @param {NodeJS.WritableStream} [opts.stderr]
 */
export async function runStdio(opts) {
  const stdin = opts.stdin || process.stdin;
  const stdout = opts.stdout || process.stdout;
  const stderr = opts.stderr || process.stderr;

  const writeOut = (obj) => {
    stdout.write(`${JSON.stringify(obj)}\n`);
  };

  const logger = new JsonlLogger({
    logDir: opts.logDir,
    clientLabel: opts.clientLabel || process.env.MCP_PROBE_CLIENT || "stdio",
  });

  const session = createSession({
    logger,
    transport: "stdio",
    sessionId: logger.sessionId,
    send: writeOut,
  });

  stderr.write(
    `[mcp-probe] stdio session=${session.sessionId} log=${logger.filePath}\n`,
  );

  const rl = readline.createInterface({ input: stdin, crlfDelay: Infinity });
  /** @type {Set<Promise<unknown>>} */
  const inflight = new Set();

  const onSignal = () => {
    onConnectionClose(session, "signal");
    try {
      rl.close();
    } catch {
      /* ignore */
    }
  };
  process.once("SIGINT", onSignal);
  process.once("SIGTERM", onSignal);

  const dispatchLine = (line) => {
    if (!line.trim()) return;
    let messages;
    try {
      messages = parseRpc(line);
    } catch (err) {
      writeOut({
        jsonrpc: "2.0",
        id: null,
        error: {
          code: -32700,
          message: `Parse error: ${err instanceof Error ? err.message : err}`,
        },
      });
      return;
    }
    for (const msg of messages) {
      const task = Promise.resolve()
        .then(() => handleMessage(session, msg))
        .then((response) => {
          if (response) writeOut(response);
        })
        .catch((err) => {
          stderr.write(
            `[mcp-probe] handler error: ${err instanceof Error ? err.message : err}\n`,
          );
        })
        .finally(() => {
          inflight.delete(task);
        });
      inflight.add(task);
    }
  };

  try {
    await new Promise((resolve) => {
      rl.on("line", dispatchLine);
      rl.on("close", resolve);
      stdin.on("end", () => {
        try {
          rl.close();
        } catch {
          /* ignore */
        }
      });
    });
    // Drain in-flight handlers (cancel/abort may still be settling).
    await Promise.allSettled([...inflight]);
  } finally {
    process.off("SIGINT", onSignal);
    process.off("SIGTERM", onSignal);
    if (!session.closed) {
      onConnectionClose(session, "stdio_eof");
    }
  }

  return { sessionId: session.sessionId, logFile: logger.filePath };
}
