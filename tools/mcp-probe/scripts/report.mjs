#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readJsonl, summarizeEvents } from "../src/logger.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * Build a markdown report from JSONL logs and/or a matrix results JSON.
 * @param {object} opts
 * @param {string} [opts.logDir]
 * @param {string} [opts.matrixPath]
 * @param {string} [opts.outPath]
 */
export function generateReport(opts = {}) {
  const sections = [];
  sections.push("# MCP Probe Report");
  sections.push("");
  sections.push(`Generated: ${new Date().toISOString()}`);
  sections.push("");

  if (opts.matrixPath && fs.existsSync(opts.matrixPath)) {
    const matrix = JSON.parse(fs.readFileSync(opts.matrixPath, "utf8"));
    sections.push("## Run matrix");
    sections.push("");
    sections.push(
      `| delay_s | progress | status | duration_ms | timeout_s | progress_token | progress_notifications | cancelled | log |`,
    );
    sections.push(
      `| ---: | :---: | --- | ---: | ---: | :---: | ---: | :---: | --- |`,
    );
    for (const row of matrix.rows || []) {
      sections.push(
        `| ${row.delay_s} | ${row.progress ? "yes" : "no"} | ${row.status} | ${
          row.duration_ms ?? "—"
        } | ${row.timeout_s ?? "—"} | ${row.progress_token_present ? "yes" : "no"} | ${
          row.progress_notifications ?? 0
        } | ${row.cancelled ? "yes" : "no"} | ${row.log_file ? path.basename(row.log_file) : "—"} |`,
      );
    }
    sections.push("");
    if (matrix.meta) {
      sections.push("### Matrix meta");
      sections.push("");
      sections.push("```json");
      sections.push(JSON.stringify(matrix.meta, null, 2));
      sections.push("```");
      sections.push("");
    }
  }

  if (opts.logDir && fs.existsSync(opts.logDir)) {
    const files = fs
      .readdirSync(opts.logDir)
      .filter((f) => f.endsWith(".jsonl"))
      .sort();
    sections.push("## Session summaries");
    sections.push("");
    if (files.length === 0) {
      sections.push("_No JSONL logs found._");
      sections.push("");
    }
    for (const file of files) {
      const full = path.join(opts.logDir, file);
      const events = readJsonl(full);
      const summary = summarizeEvents(events);
      sections.push(`### ${file}`);
      sections.push("");
      sections.push(
        `| field | value |`,
      );
      sections.push(`| --- | --- |`);
      sections.push(`| started_at | ${summary.started_at ?? "—"} |`);
      sections.push(`| ended_at | ${summary.ended_at ?? "—"} |`);
      sections.push(
        `| clientInfo | \`${JSON.stringify(summary.clientInfo) ?? "null"}\` |`,
      );
      sections.push(
        `| capabilities | \`${JSON.stringify(summary.capabilities) ?? "null"}\` |`,
      );
      sections.push(`| protocolVersion | ${summary.protocolVersion ?? "—"} |`);
      sections.push(
        `| progress_token_supported | ${summary.progress_token_supported ? "yes" : "no"} |`,
      );
      sections.push(`| cancelled_count | ${summary.cancelled_count} |`);
      sections.push(`| connection_close_count | ${summary.connection_close_count} |`);
      sections.push(
        `| connection_reconnect_count | ${summary.connection_reconnect_count} |`,
      );
      sections.push(
        `| timeout_seconds | ${
          summary.timeout_seconds.length
            ? summary.timeout_seconds.join(", ")
            : "—"
        } |`,
      );
      sections.push("");
      if (summary.calls.length) {
        sections.push(
          `| tool | status | delay_ms | duration_ms | progress_token | progress_n |`,
        );
        sections.push(`| --- | --- | ---: | ---: | :---: | ---: |`);
        for (const c of summary.calls) {
          sections.push(
            `| ${c.tool} | ${c.status} | ${c.delay_ms ?? "—"} | ${
              c.duration_ms ?? "—"
            } | ${c.progress_token_present ? "yes" : "no"} | ${
              c.progress_notifications_sent
            } |`,
          );
        }
        sections.push("");
      }
    }
  }

  sections.push("## Notes");
  sections.push("");
  sections.push(
    "- Real GUI/CLI client measurements are performed by a human against this probe; CI only exercises the fake client harness.",
  );
  sections.push(
    "- `timeout_s` / `connection_close` during an in-flight `slow_echo` indicate the client dropped the call (tool timeout or disconnect).",
  );
  sections.push(
    "- See `docs/qa/mcp-probe.md` for per-client MCP config examples.",
  );
  sections.push("");

  const markdown = sections.join("\n");
  if (opts.outPath) {
    fs.mkdirSync(path.dirname(path.resolve(opts.outPath)), { recursive: true });
    fs.writeFileSync(opts.outPath, markdown, "utf8");
  }
  return markdown;
}

function parseCli(argv) {
  const out = {
    "log-dir": path.resolve(__dirname, "../logs"),
    matrix: "",
    out: path.resolve(__dirname, "../logs/report.md"),
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--") && argv[i + 1] && !argv[i + 1].startsWith("--")) {
      out[a.slice(2)] = argv[++i];
    }
  }
  return out;
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (isMain) {
  const args = parseCli(process.argv.slice(2));
  const md = generateReport({
    logDir: args["log-dir"],
    matrixPath: args.matrix || undefined,
    outPath: args.out,
  });
  process.stdout.write(md);
  if (args.out) {
    process.stderr.write(`[mcp-probe] wrote ${args.out}\n`);
  }
}
