import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const EXT_CANDIDATES = ["", ".ts", ".tsx", ".js", ".mjs", ".json"];

/** @type {string} */
let root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");

/** @param {{ root?: string }} data */
export async function initialize(data) {
  if (data?.root) {
    root = data.root;
  }
}

/**
 * @param {string} specifier
 * @param {object} context
 * @param {(specifier: string, context: object) => Promise<object>} nextResolve
 */
export async function resolve(specifier, context, nextResolve) {
  if (!specifier.startsWith("@/")) {
    return nextResolve(specifier, context);
  }

  const base = path.join(root, "src", specifier.slice(2));

  for (const ext of EXT_CANDIDATES) {
    const candidate = base + ext;
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
      return {
        shortCircuit: true,
        url: pathToFileURL(candidate).href,
      };
    }
  }

  return {
    shortCircuit: true,
    url: pathToFileURL(base).href,
  };
}
