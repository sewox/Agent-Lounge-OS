#!/usr/bin/env node
import { main } from "../src/cli.mjs";

main().then(
  (code) => {
    if (typeof code === "number" && code !== 0) process.exit(code);
  },
  (err) => {
    console.error(err);
    process.exit(1);
  },
);
