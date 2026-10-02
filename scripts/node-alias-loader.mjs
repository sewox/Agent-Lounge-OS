/**
 * Registers an ESM resolve hook so `node --experimental-strip-types`
 * can load `@/*` the same way tsconfig paths do. Used by `npm run test:unit`.
 */
import path from "node:path";
import { register } from "node:module";

const root = path.resolve(import.meta.dirname, "..");

register("./node-alias-resolve.mjs", import.meta.url, {
  data: { root },
});
